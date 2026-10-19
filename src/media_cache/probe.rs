use std::{path::PathBuf, process::Stdio, time::Duration};

use serde_json::Value;
use tokio::{io::AsyncBufReadExt, process::Command, sync::mpsc};

#[derive(Debug, Clone)]
pub struct YtDlpEnv {
    pub binary: String,
    pub cookies: Option<PathBuf>,
    pub format: String,
    pub sub_langs: String,
    pub stall_timeout: Duration,
    /// -1 means unlimited.
    pub max_size: i64,
}

#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub extractor: String,
    pub id: String,
    pub title: Option<String>,
    pub webpage_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FlatEntry {
    pub url: String,
    pub id: Option<String>,
    pub ie_key: Option<String>,
    pub title: Option<String>,
}

/// One entry of a streaming probe; `None` fields were reported as `NA`.
#[derive(Debug, Clone)]
pub struct StreamedEntry {
    pub url: String,
    pub title: Option<String>,
    pub id: Option<String>,
    pub extractor: Option<String>,
    pub duration: Option<f64>,
    pub live_status: Option<String>,
    /// Total size the source reports for the playlist, if any.
    pub playlist_count: Option<u64>,
}

#[derive(Debug)]
pub enum ProbeOutcome {
    /// Not cacheable; mpv streams it instead.
    Passthrough,
    Single(VideoInfo),
    /// `total` is the playlist size the source reports, if any.
    Playlist {
        entries: Vec<FlatEntry>,
        total: Option<u64>,
    },
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(120);

/// Metadata-only probe; any error maps to `Passthrough`.
pub async fn probe(env: &YtDlpEnv, url: &str) -> ProbeOutcome {
    let mut command = Command::new(&env.binary);
    command
        .arg("-J")
        .arg("--flat-playlist")
        .arg("--no-warnings")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cookies) = &env.cookies {
        command.arg("--cookies").arg(cookies);
    }
    command.arg(url);

    let output = match tokio::time::timeout(PROBE_TIMEOUT, async {
        let child = command.spawn();
        match child {
            Ok(child) => child.wait_with_output().await,
            Err(err) => Err(err),
        }
    })
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => {
            tracing::warn!("Failed to run yt-dlp probe for {url}: {err}");
            return ProbeOutcome::Passthrough;
        }
        Err(_) => {
            tracing::warn!("yt-dlp probe for {url} timed out");
            return ProbeOutcome::Passthrough;
        }
    };

    if !output.status.success() {
        tracing::debug!(
            "yt-dlp probe for {url} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return ProbeOutcome::Passthrough;
    }

    match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(value) => parse_probe(&value),
        Err(err) => {
            tracing::warn!("Failed to parse yt-dlp probe output for {url}: {err}");
            ProbeOutcome::Passthrough
        }
    }
}

/// Streams entries of a `--flat-playlist --print` run as they are parsed,
/// instead of waiting for the whole playlist.
pub fn spawn_stream_probe(env: &YtDlpEnv, url: &str) -> mpsc::UnboundedReceiver<StreamedEntry> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut command = Command::new(&env.binary);
    command
        .arg("--flat-playlist")
        .arg("--no-warnings")
        .arg("--print")
        .arg(STREAM_TEMPLATE)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cookies) = &env.cookies {
        command.arg("--cookies").arg(cookies);
    }
    command.arg(url);

    let url = url.to_string();
    tokio::spawn(async move {
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!("Failed to run yt-dlp probe for {url}: {err}");
                return;
            }
        };
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::trace!("yt-dlp stderr: {line}");
                }
            });
        }

        let read_entries = async {
            let stdout = child.stdout.take().expect("stdout is piped");
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(entry) = parse_streamed(&line)
                    && !is_live_status(&entry.live_status)
                    && tx.send(entry).is_ok()
                {}
            }
        };

        if tokio::time::timeout(PROBE_TIMEOUT, read_entries)
            .await
            .is_err()
        {
            tracing::warn!("yt-dlp probe for {url} timed out");
        }
        let _ = child.wait().await;
    });
    rx
}

// url, id, extractor, duration, live_status, playlist_count, title; the
// title goes last so it may contain tabs. Alternatives pick the first
// non-NA field.
const STREAM_TEMPLATE: &str = "%(webpage_url,url)s\t%(id)s\t%(extractor,ie_key)s\t%(duration)s\t%(live_status)s\t%(playlist_count)s\t%(title)s";

fn parse_streamed(line: &str) -> Option<StreamedEntry> {
    let mut parts = line.splitn(7, '\t');
    let url = parts.next()?.trim();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return None;
    }
    let id = field(parts.next()?);
    let extractor = field(parts.next()?);
    let duration = parts.next()?.trim().parse().ok();
    let live_status = field(parts.next()?);
    let playlist_count = parts.next()?.trim().parse().ok();
    let title = field(parts.next()?);
    Some(StreamedEntry {
        url: url.to_string(),
        title,
        id,
        extractor,
        duration,
        live_status,
        playlist_count,
    })
}

fn field(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value == "NA" {
        None
    } else {
        Some(value.to_string())
    }
}

fn is_live_status(status: &Option<String>) -> bool {
    matches!(
        status.as_deref(),
        Some("is_live") | Some("post_live") | Some("in_progress")
    )
}

fn parse_probe(value: &Value) -> ProbeOutcome {
    if value.is_null() {
        return ProbeOutcome::Passthrough;
    }

    match value.get("_type").and_then(Value::as_str) {
        Some("playlist") => ProbeOutcome::Playlist {
            entries: parse_flat_entries(value),
            total: value.get("playlist_count").and_then(Value::as_u64),
        },
        _ => parse_single(value),
    }
}

fn parse_flat_entries(value: &Value) -> Vec<FlatEntry> {
    value
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let url = entry.get("url").and_then(Value::as_str)?;
                    if is_live(entry) {
                        return None;
                    }
                    Some(FlatEntry {
                        url: url.to_string(),
                        id: entry.get("id").and_then(Value::as_str).map(str::to_string),
                        ie_key: entry
                            .get("ie_key")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        title: str_field(entry, "title"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_single(value: &Value) -> ProbeOutcome {
    if is_live(value) {
        return ProbeOutcome::Passthrough;
    }

    let Some(extractor) = str_field(value, "extractor")
        .or_else(|| str_field(value, "extractor_key").map(|key| key.to_lowercase()))
    else {
        return ProbeOutcome::Passthrough;
    };
    let Some(id) = str_field(value, "id") else {
        return ProbeOutcome::Passthrough;
    };
    if num_field(value, "duration").is_none() {
        return ProbeOutcome::Passthrough;
    }

    ProbeOutcome::Single(VideoInfo {
        extractor,
        id,
        title: str_field(value, "title"),
        webpage_url: str_field(value, "webpage_url"),
    })
}

fn is_live(value: &Value) -> bool {
    if value.get("is_live").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    matches!(
        value.get("live_status").and_then(Value::as_str),
        Some("is_live") | Some("post_live") | Some("in_progress")
    )
}

fn str_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

fn num_field(value: &Value, field: &str) -> Option<f64> {
    value.get(field).and_then(Value::as_f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn streamed_entries_parse_with_na_and_tabs() {
        let entry = parse_streamed(
            "https://youtu.be/x\tabc123\tYoutube\t212.5\twas_live\t182\tTitle,\twith tabs",
        )
        .expect("valid line");
        assert_eq!(entry.url, "https://youtu.be/x");
        assert_eq!(entry.id.as_deref(), Some("abc123"));
        assert_eq!(entry.extractor.as_deref(), Some("Youtube"));
        assert_eq!(entry.duration, Some(212.5));
        assert_eq!(entry.live_status.as_deref(), Some("was_live"));
        assert_eq!(entry.playlist_count, Some(182));
        assert_eq!(entry.title.as_deref(), Some("Title,\twith tabs"));

        let entry = parse_streamed("https://x/y\tNA\tNA\tNA\tNA\tNA\tNA").expect("valid line");
        assert_eq!(entry.id, None);
        assert_eq!(entry.duration, None);
        assert_eq!(entry.playlist_count, None);
        assert_eq!(entry.title, None);

        assert!(parse_streamed("NA\tNA\tNA\tNA\tNA\tNA\tNA").is_none());
        assert!(parse_streamed("not-a-url\t1\t2\t3\t4\t5\t6").is_none());
        assert!(parse_streamed("https://x/y\ttoofew").is_none());
    }

    #[test]
    fn live_statuses_are_filtered() {
        for status in ["is_live", "post_live", "in_progress"] {
            assert!(is_live_status(&Some(status.to_string())));
        }
        assert!(!is_live_status(&Some("was_live".to_string())));
        assert!(!is_live_status(&None));
    }

    #[test]
    fn null_output_is_passthrough() {
        assert!(matches!(
            parse_probe(&Value::Null),
            ProbeOutcome::Passthrough
        ));
    }

    #[test]
    fn single_video_parses_fields() {
        let value = json!({
            "_type": "video",
            "extractor": "youtube",
            "id": "abc123",
            "title": "Test Video",
            "duration": 42.5,
            "webpage_url": "https://www.youtube.com/watch?v=abc123",
        });
        let ProbeOutcome::Single(info) = parse_probe(&value) else {
            panic!("expected single video");
        };
        assert_eq!(info.extractor, "youtube");
        assert_eq!(info.id, "abc123");
        assert_eq!(
            info.webpage_url.as_deref(),
            Some("https://www.youtube.com/watch?v=abc123")
        );
    }

    #[test]
    fn live_or_unknown_duration_is_passthrough() {
        let live = json!({"_type": "video", "extractor": "youtube", "id": "x", "live_status": "is_live", "duration": 10.0});
        assert!(matches!(parse_probe(&live), ProbeOutcome::Passthrough));

        let no_duration = json!({"_type": "video", "extractor": "generic", "id": "x"});
        assert!(matches!(
            parse_probe(&no_duration),
            ProbeOutcome::Passthrough
        ));
    }

    #[test]
    fn playlists_parse_flat_entries_and_skip_live() {
        let value = json!({
            "_type": "playlist",
            "id": "PL123",
            "entries": [
                {"_type": "url", "id": "a", "url": "https://www.youtube.com/watch?v=a", "title": "A", "duration": 10.0},
                {"_type": "url", "id": "b", "url": "https://www.youtube.com/watch?v=b", "live_status": "is_live"},
                {"_type": "url", "id": "c", "title": "no url"}
            ]
        });
        let ProbeOutcome::Playlist { entries, .. } = parse_probe(&value) else {
            panic!("expected playlist");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].url, "https://www.youtube.com/watch?v=a");
    }
}

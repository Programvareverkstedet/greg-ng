use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, bail};
use futures::{Stream, stream::try_unfold};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader, Lines},
    process::{Child, ChildStdout, Command},
    task::JoinHandle,
};

const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_STDERR_LEN: usize = 500;

const ENTRY_PRINT_TEMPLATE: &str = "%(webpage_url)j\t%(title|)j";

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedEntry {
    pub url: String,
    pub title: Option<String>,
}

/// Expand a URL into a stream of entries with their titles. Nothing
/// runs until the first poll.
pub fn expand_entries(
    url: String,
    cookies_path: PathBuf,
) -> impl Stream<Item = anyhow::Result<ExpandedEntry>> + Send {
    try_unfold(ExpandState::Spawn { url, cookies_path }, expand_step)
}

async fn expand_step(state: ExpandState) -> anyhow::Result<Option<(ExpandedEntry, ExpandState)>> {
    let mut state = state;
    // Spawning and skipped lines yield no items, so loop until one does
    loop {
        match std::mem::replace(&mut state, ExpandState::Done) {
            ExpandState::Done => return Ok(None),

            ExpandState::Spawn { url, cookies_path } => {
                state = spawn_entry_stream(url, cookies_path)?;
            }

            ExpandState::Entries(mut entries) => {
                let line = next_line(&mut entries.lines).await?;

                let Some(line) = line else {
                    wait_for_exit(&mut entries.child, entries.stderr_task).await?;
                    return Ok(None);
                };

                let Some(entry) = parse_entry_line(&line) else {
                    state = ExpandState::Entries(entries);
                    continue;
                };

                return Ok(Some((entry, ExpandState::Entries(entries))));
            }
        }
    }
}

fn parse_entry_line(line: &str) -> Option<ExpandedEntry> {
    let (url, title) = line.trim_end().split_once('\t')?;
    let url: String = serde_json::from_str(url.trim()).ok()?;
    if url.is_empty() {
        return None;
    }

    let title: Option<String> = serde_json::from_str(title.trim()).ok()?;
    Some(ExpandedEntry {
        url,
        title: title.filter(|title| !title.is_empty()),
    })
}

enum ExpandState {
    Spawn { url: String, cookies_path: PathBuf },
    Entries(Box<EntriesState>),
    Done,
}

struct EntriesState {
    lines: Lines<BufReader<ChildStdout>>,
    child: Child,
    stderr_task: JoinHandle<Vec<u8>>,
}

fn spawn_entry_stream(url: String, cookies_path: PathBuf) -> anyhow::Result<ExpandState> {
    let mut child = Command::new("yt-dlp")
        .arg("--no-warnings")
        .arg("--cookies")
        .arg(&cookies_path)
        .args([
            "--lazy-playlist",
            "--flat-playlist",
            "--yes-playlist",
            "--print",
            ENTRY_PRINT_TEMPLATE,
        ])
        .arg("--extractor-args")
        .arg("youtube:skip=hls,dash")
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Failed to spawn yt-dlp")?;

    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr_task = {
        let mut stderr = child.stderr.take().expect("stderr is piped");
        // Drain stderr concurrently so the child never blocks on a full pipe
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf).await;
            buf
        })
    };

    Ok(ExpandState::Entries(Box::new(EntriesState {
        lines: BufReader::new(stdout).lines(),
        child,
        stderr_task,
    })))
}

async fn next_line(lines: &mut Lines<BufReader<ChildStdout>>) -> anyhow::Result<Option<String>> {
    tokio::time::timeout(STREAM_IDLE_TIMEOUT, lines.next_line())
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "yt-dlp stalled for {} seconds",
                STREAM_IDLE_TIMEOUT.as_secs()
            )
        })?
        .context("Failed to read yt-dlp output")
}

async fn wait_for_exit(child: &mut Child, stderr_task: JoinHandle<Vec<u8>>) -> anyhow::Result<()> {
    let status = tokio::time::timeout(STREAM_IDLE_TIMEOUT, child.wait())
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "yt-dlp did not exit within {} seconds after end of output",
                STREAM_IDLE_TIMEOUT.as_secs()
            )
        })?
        .context("Failed to wait for yt-dlp")?;

    let stderr = stderr_task.await.unwrap_or_default();
    if !status.success() {
        bail!("yt-dlp failed with {}: {}", status, stderr_tail(&stderr));
    }

    Ok(())
}

fn stderr_tail(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    if stderr.chars().count() > MAX_STDERR_LEN {
        let skip = stderr.chars().count() - MAX_STDERR_LEN;
        stderr.chars().skip(skip).collect::<String>()
    } else {
        stderr.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_line_parses_url_and_title() {
        assert_eq!(
            parse_entry_line("\"https://example.com/1\"\t\"A Title\""),
            Some(ExpandedEntry {
                url: "https://example.com/1".to_string(),
                title: Some("A Title".to_string()),
            })
        );
    }

    #[test]
    fn entry_line_title_is_optional() {
        assert_eq!(
            parse_entry_line("\"https://example.com/1\"\t\"\""),
            Some(ExpandedEntry {
                url: "https://example.com/1".to_string(),
                title: None,
            })
        );
    }

    #[test]
    fn entry_line_survives_control_characters_in_title() {
        assert_eq!(
            parse_entry_line("\"https://example.com/1\"\t\"Bad\\nTitle\\tHere\""),
            Some(ExpandedEntry {
                url: "https://example.com/1".to_string(),
                title: Some("Bad\nTitle\tHere".to_string()),
            })
        );
    }

    #[test]
    fn entry_line_without_url_is_skipped() {
        assert_eq!(parse_entry_line("\"\t\"Only a title\""), None);
        assert_eq!(parse_entry_line(""), None);
        assert_eq!(parse_entry_line("not json at all"), None);
    }
}

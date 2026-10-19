use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::Context;

static FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// One playlist entry to hand to mpv.
#[derive(Debug, Clone)]
pub struct Entry {
    pub filename: String,
    pub title: Option<String>,
}

/// A temporary M3U file, removed when dropped.
pub struct TempM3u {
    path: PathBuf,
}

impl TempM3u {
    pub fn path(&self) -> &str {
        self.path.to_str().expect("tmp path is valid UTF-8")
    }
}

impl Drop for TempM3u {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Writes `entries` as an M3U playlist so mpv's own parser attaches the
/// titles to the playlist entries. Filenames must not start with `#`
/// (true for URLs and cache paths by construction).
pub fn write(dir: &Path, entries: &[Entry]) -> anyhow::Result<TempM3u> {
    fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    let unique = FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("insert-{}-{unique}.m3u", std::process::id()));

    let mut contents = String::from("#EXTM3U\n");
    for entry in entries {
        let filename = sanitize(&entry.filename);
        if let Some(title) = &entry.title {
            contents.push_str(&format!("#EXTINF:0,{}\n", sanitize(title)));
        }
        contents.push_str(&filename);
        contents.push('\n');
    }

    fs::write(&path, contents).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(TempM3u { path })
}

fn sanitize(value: &str) -> String {
    value.replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_creates_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        let _file = write(
            &missing,
            &[Entry {
                filename: "https://x/y".to_string(),
                title: Some("T".to_string()),
            }],
        )
        .unwrap();

        assert!(missing.is_dir());
    }

    fn read(dir: &Path) -> String {
        let mut files: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|e| e == "m3u"))
            .map(|e| e.path())
            .collect();
        assert_eq!(files.len(), 1);
        fs::read_to_string(files.remove(0)).unwrap()
    }

    #[test]
    fn writes_extinf_titles_and_untitled_entries() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(
            dir.path(),
            &[
                Entry {
                    filename: "https://x/y?a=1&b=2".to_string(),
                    title: Some("Title, with comma".to_string()),
                },
                Entry {
                    filename: "/tmp/a b.mkv".to_string(),
                    title: None,
                },
            ],
        )
        .unwrap();

        assert_eq!(
            read(dir.path()),
            "#EXTM3U\n#EXTINF:0,Title, with comma\nhttps://x/y?a=1&b=2\n/tmp/a b.mkv\n"
        );
        drop(file);
        assert!(fs::read_dir(dir).unwrap().next().is_none());
    }

    #[test]
    fn strips_newlines_from_titles_and_filenames() {
        let dir = tempfile::tempdir().unwrap();
        let _file = write(
            dir.path(),
            &[Entry {
                filename: "a\nb.mkv".to_string(),
                title: Some("one\ntwo\rthree".to_string()),
            }],
        )
        .unwrap();

        assert_eq!(
            read(dir.path()),
            "#EXTM3U\n#EXTINF:0,one two three\na b.mkv\n"
        );
    }
}

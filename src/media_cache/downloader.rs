use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Instant,
};

use anyhow::Context;
use sha2::{Digest, Sha256};
use tokio::io::AsyncBufReadExt;

use super::{
    key::{canonical_key, hex},
    meta::{MetaEntry, SCHEMA_VERSION},
    probe::VideoInfo,
    store::{CacheEntry, MediaCacheStore},
    unix_now,
};

const SIZE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

pub struct DownloadJob {
    pub url: String,
    pub info: VideoInfo,
}

pub enum DownloadOutcome {
    Cached(Arc<CacheEntry>),
    Skipped(String),
}

struct PrintedOutput {
    filepath: PathBuf,
    extractor: String,
    id: String,
}

/// Downloads one video; partial downloads are never kept.
pub async fn download(
    env: &super::probe::YtDlpEnv,
    store: &MediaCacheStore,
    job: DownloadJob,
) -> DownloadOutcome {
    let key = canonical_key(&job.info.extractor, &job.info.id);

    if let Some(existing) = store.lookup_key(&key) {
        store.add_url_alias(&key, &job.url);
        tracing::debug!("Cache hit for {key}, skipping download");
        return DownloadOutcome::Cached(existing);
    }

    let tmp_dir = store.tmp_root().join(&key);
    if let Err(err) = fs::remove_dir_all(&tmp_dir)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("Failed to clean up {}: {err}", tmp_dir.display());
    }
    if let Err(err) = fs::create_dir_all(&tmp_dir) {
        return DownloadOutcome::Skipped(format!("failed to create tmp dir: {err}"));
    }

    let result = run_ytdlp(env, &tmp_dir, &job.url).await;
    match result {
        Ok(Some(printed)) => finalize(env, store, &tmp_dir, &key, &job, printed).await,
        Ok(None) => {
            wipe_dir(&tmp_dir);
            DownloadOutcome::Skipped("no output produced (skipped or failed)".to_string())
        }
        Err(reason) => {
            wipe_dir(&tmp_dir);
            DownloadOutcome::Skipped(reason)
        }
    }
}

async fn run_ytdlp(
    env: &super::probe::YtDlpEnv,
    tmp_dir: &Path,
    url: &str,
) -> Result<Option<PrintedOutput>, String> {
    let output_template = format!("{}/%(id)s.%(ext)s", tmp_dir.display());
    let mut command = tokio::process::Command::new(&env.binary);
    command
        .args([
            "--no-playlist",
            "--match-filter",
            "!is_live",
            "-f",
            env.format.as_str(),
            "--merge-output-format",
            "mkv",
            "--embed-metadata",
            "--no-embed-info-json",
            "--embed-thumbnail",
            "--write-subs",
            "--sub-langs",
            env.sub_langs.as_str(),
            "--write-thumbnail",
            "--convert-thumbnails",
            "jpg",
            "--write-info-json",
            "--write-description",
            "--newline",
            "--progress",
            "--no-simulate",
            "--ignore-errors",
            "--retries",
            "10",
            "--fragment-retries",
            "10",
            "--print",
            "after_move:filepath",
            "--print",
            "after_move:%(extractor)s",
            "--print",
            "after_move:%(id)s",
            "-o",
            output_template.as_str(),
        ])
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cookies) = &env.cookies {
        command.arg("--cookies").arg(cookies);
    }
    command.arg(url);

    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to spawn yt-dlp: {err}"))?;

    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::trace!("yt-dlp stderr: {line}");
            }
        });
    }

    let stdout = child.stdout.take().expect("stdout is piped");
    let mut lines = tokio::io::BufReader::new(stdout).lines();

    let mut printed: Vec<String> = Vec::new();
    let mut last_activity = Instant::now();
    let mut prev_size = dir_size(tmp_dir);
    let mut size_poll = tokio::time::interval(SIZE_POLL_INTERVAL);
    size_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let stalled = loop {
        let deadline = last_activity + env.stall_timeout;
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        if line.starts_with("[download]") {
                            tracing::trace!("yt-dlp: {line}");
                        } else if !line.trim().is_empty() && !line.starts_with('[') {
                            printed.push(line.trim().to_string());
                        }
                        last_activity = Instant::now();
                    }
                    Ok(None) => break false,
                    Err(err) => {
                        return Err(format!("failed to read yt-dlp output: {err}"));
                    }
                }
            }
            _ = size_poll.tick() => {
                let size = dir_size(tmp_dir);
                if size != prev_size {
                    prev_size = size;
                    last_activity = Instant::now();
                }
            }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                break true;
            }
        }
    };

    if stalled {
        let _ = child.kill().await;
        return Err(format!(
            "download stalled: no output or growth for {} seconds",
            env.stall_timeout.as_secs()
        ));
    }

    let status = child
        .wait()
        .await
        .map_err(|err| format!("failed to wait for yt-dlp: {err}"))?;

    if printed.len() != 3 {
        tracing::debug!(
            "yt-dlp exited with {status} and {} printable lines: {printed:?}",
            printed.len()
        );
        if !status.success() {
            return Err(format!("yt-dlp failed with status {status}"));
        }
        return Ok(None);
    }

    let filepath = PathBuf::from(&printed[0]);
    if !filepath.exists() {
        return Err(format!(
            "yt-dlp reported {:?} but the file is missing",
            printed[0]
        ));
    }

    Ok(Some(PrintedOutput {
        filepath,
        extractor: printed[1].clone(),
        id: printed[2].clone(),
    }))
}

async fn finalize(
    env: &super::probe::YtDlpEnv,
    store: &MediaCacheStore,
    tmp_dir: &Path,
    expected_key: &str,
    job: &DownloadJob,
    printed: PrintedOutput,
) -> DownloadOutcome {
    match finalize_inner(env, store, tmp_dir, expected_key, job, printed).await {
        Ok(outcome) => outcome,
        Err(reason) => {
            wipe_dir(tmp_dir);
            DownloadOutcome::Skipped(reason.to_string())
        }
    }
}

async fn finalize_inner(
    env: &super::probe::YtDlpEnv,
    store: &MediaCacheStore,
    tmp_dir: &Path,
    expected_key: &str,
    job: &DownloadJob,
    printed: PrintedOutput,
) -> anyhow::Result<DownloadOutcome> {
    let key = canonical_key(&printed.extractor, &printed.id);

    if key != expected_key
        && let Some(existing) = store.lookup_key(&key)
    {
        store.add_url_alias(&key, &job.url);
        wipe_dir(tmp_dir);
        tracing::debug!("Variant of {key} already cached");
        return Ok(DownloadOutcome::Cached(existing));
    }

    let size = fs::metadata(&printed.filepath)
        .with_context(|| format!("failed to stat download {}", printed.filepath.display()))?
        .len();
    anyhow::ensure!(
        env.max_size < 0 || size <= env.max_size as u64,
        "download of {size} bytes exceeds max_size of {} bytes",
        env.max_size
    );

    let hash_path = printed.filepath.clone();
    let content_hash = tokio::task::spawn_blocking(move || sha256_file(&hash_path))
        .await
        .map_err(|err| anyhow::anyhow!("hashing task failed: {err}"))??;

    let entry_dir = store.entry_dir(&key);
    fs::create_dir_all(&entry_dir)
        .with_context(|| format!("failed to create entry dir {}", entry_dir.display()))?;

    let media_file = printed
        .filepath
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .context("download has no valid file name")?;

    let mut deduped_size = None;
    if let Some(older) = store.lookup_content(&content_hash)
        && older.meta.key != key
    {
        match fs::hard_link(older.media_path(), entry_dir.join(&media_file)) {
            Ok(()) => deduped_size = Some(older.meta.size),
            Err(err) => tracing::warn!(
                "Failed to hardlink duplicate content for {key}: {err}, storing copy"
            ),
        }
    }

    let sidecar_failures = if deduped_size.is_none() {
        let src = tmp_dir.to_path_buf();
        let dst = entry_dir.clone();
        let failures = tokio::task::spawn_blocking(move || move_files(&src, &dst, None))
            .await
            .map_err(|err| anyhow::anyhow!("failed to move download into cache: {err}"))??;
        if !failures.is_empty() {
            anyhow::bail!("failed to move download into cache: {}", failures[0]);
        }
        Vec::new()
    } else {
        let failures = move_files(tmp_dir, &entry_dir, Some(&media_file)).unwrap_or_default();
        wipe_dir(tmp_dir);
        failures
    };
    for err in sidecar_failures {
        tracing::warn!("Failed to move sidecar file: {err}");
    }

    let final_size = deduped_size.unwrap_or(size);

    let entry = CacheEntry {
        dir: entry_dir.clone(),
        meta: build_meta(job, &key, &media_file, final_size, content_hash),
    };

    match store.insert(entry) {
        Ok(inserted) => {
            tracing::info!("Cached {key} ({} bytes) from {:?}", final_size, job.url);
            Ok(DownloadOutcome::Cached(inserted))
        }
        Err(err) => {
            wipe_dir(&entry_dir);
            anyhow::bail!("failed to register entry: {err}");
        }
    }
}

fn build_meta(
    job: &DownloadJob,
    key: &str,
    media_file: &str,
    size: u64,
    content_hash: String,
) -> MetaEntry {
    let info = &job.info;
    let mut urls = Vec::new();
    if let Some(webpage_url) = info.webpage_url.as_deref()
        && !urls.iter().any(|u| u == webpage_url)
    {
        urls.push(webpage_url.to_string());
    }
    if !urls.iter().any(|u| u == &job.url) {
        urls.push(job.url.clone());
    }

    let now = unix_now();
    MetaEntry {
        schema_version: SCHEMA_VERSION,
        key: key.to_string(),
        urls,
        title: info.title.clone(),
        media_file: media_file.to_string(),
        size,
        content_hash: Some(content_hash),
        created_at: now,
        last_used: now,
    }
}

/// Evicts LRU entries until the size policy holds; playlist-referenced files are pinned.
pub fn evict(
    store: &MediaCacheStore,
    max_size: i64,
    min_free_space: u64,
    pinned: &HashSet<PathBuf>,
) -> Vec<String> {
    let mut evicted = Vec::new();
    loop {
        let over_budget = max_size >= 0 && store.total_size() > max_size as u64;
        let low_disk = match free_space(store.root()) {
            Some(free) => free < min_free_space,
            None => {
                tracing::warn!(
                    "Failed to determine free space on {}, skipping the free space floor",
                    store.root().display()
                );
                false
            }
        };
        if !over_budget && !low_disk {
            break;
        }

        let Some(candidate) = store
            .entries_by_last_used()
            .into_iter()
            .find(|entry| !pinned.contains(&entry.media_path()))
        else {
            tracing::warn!("Cache is over budget but all remaining entries are pinned");
            break;
        };

        tracing::info!(
            "Evicting {} ({})",
            candidate.meta.key,
            candidate.meta.title.as_deref().unwrap_or("?")
        );
        let key = candidate.meta.key.clone();
        match store.remove(&key) {
            Some(_) => evicted.push(key),
            None => break,
        }
    }
    evicted
}

fn free_space(path: &Path) -> Option<u64> {
    nix::sys::statvfs::statvfs(path)
        .ok()
        .map(|stats| stats.blocks_available() * stats.fragment_size())
}

fn wipe_dir(dir: &Path) {
    if let Err(err) = fs::remove_dir_all(dir)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("Failed to remove {}: {err}", dir.display());
    }
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(children) = fs::read_dir(dir) {
        for child in children.flatten() {
            let path = child.path();
            if path.is_dir() {
                total += dir_size(&path);
            } else if let Ok(metadata) = child.metadata() {
                total += metadata.len();
            }
        }
    }
    total
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Moves all files except `skip`; returns the per-file failures.
fn move_files(
    src_dir: &Path,
    dst_dir: &Path,
    skip: Option<&str>,
) -> anyhow::Result<Vec<anyhow::Error>> {
    let mut failures = Vec::new();
    for child in fs::read_dir(src_dir)
        .with_context(|| format!("Failed to read {}", src_dir.display()))?
        .flatten()
    {
        let name = child.file_name();
        if skip == Some(name.to_string_lossy().as_ref()) {
            continue;
        }
        if let Err(err) = move_file(&child.path(), &dst_dir.join(&name)) {
            failures.push(err);
        }
    }
    Ok(failures)
}

fn move_file(src: &Path, dst: &Path) -> anyhow::Result<()> {
    if let Err(err) = fs::rename(src, dst) {
        if err.kind() == std::io::ErrorKind::CrossesDevices {
            fs::copy(src, dst).with_context(|| {
                format!("Failed to copy {} to {}", src.display(), dst.display())
            })?;
            fs::remove_file(src)?;
            return Ok(());
        }
        return Err(err)
            .with_context(|| format!("Failed to move {} to {}", src.display(), dst.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evict_removes_lru_and_respects_pins() {
        let root = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store = MediaCacheStore::open(root.path(), tmp.path()).unwrap();

        let mut old =
            crate::media_cache::store::tests::sample_entry("old", &["https://x/old"], 100);
        old.meta.size = 100;
        old.dir = store.entry_dir("old");
        let mut mid =
            crate::media_cache::store::tests::sample_entry("mid", &["https://x/mid"], 200);
        mid.meta.size = 100;
        mid.dir = store.entry_dir("mid");
        let mut young =
            crate::media_cache::store::tests::sample_entry("young", &["https://x/young"], 300);
        young.meta.size = 100;
        young.dir = store.entry_dir("young");

        store.insert(old).unwrap();
        store.insert(mid).unwrap();
        store.insert(young).unwrap();
        fs::write(store.entry_dir("old").join("video.mkv"), b"x").unwrap();
        fs::write(store.entry_dir("mid").join("video.mkv"), b"x").unwrap();
        fs::write(store.entry_dir("young").join("video.mkv"), b"x").unwrap();

        let evicted = evict(&store, 250, 0, &HashSet::new());
        assert_eq!(evicted, vec!["old".to_string()]);
        assert_eq!(store.total_size(), 200);

        let pinned: HashSet<PathBuf> = [store.entry_dir("mid").join("video.mkv")].into();
        let evicted = evict(&store, 50, 0, &pinned);
        assert_eq!(evicted, vec!["young".to_string()]);
        assert_eq!(store.total_size(), 100);
    }

    #[test]
    fn evict_unlimited_only_reacts_to_low_disk() {
        let root = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store = MediaCacheStore::open(root.path(), tmp.path()).unwrap();

        let mut entry =
            crate::media_cache::store::tests::sample_entry("free", &["https://x/free"], 100);
        entry.meta.size = u64::MAX;
        entry.dir = store.entry_dir("free");
        store.insert(entry).unwrap();
        fs::write(store.entry_dir("free").join("video.mkv"), b"x").unwrap();

        assert!(evict(&store, -1, 0, &HashSet::new()).is_empty());

        let free = free_space(root.path()).expect("statvfs on tempdir");
        let evicted = evict(&store, -1, free + 1024, &HashSet::new());
        assert_eq!(evicted, vec!["free".to_string()]);
        assert_eq!(store.total_size(), 0);
    }

    #[test]
    fn sha256_file_matches_known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn move_file_falls_back_to_copy_across_devices() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::write(&src, b"content").unwrap();
        move_file(&src, &dst).unwrap();
        assert!(!src.exists());
        assert_eq!(fs::read(&dst).unwrap(), b"content");
    }

    #[test]
    fn move_files_skips_named_file() {
        let dir = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("video.mkv"), b"media").unwrap();
        fs::write(dir.path().join("video.en.vtt"), b"subs").unwrap();
        move_files(dir.path(), dst.path(), Some("video.mkv")).unwrap();
        assert!(dir.path().join("video.mkv").exists());
        assert!(dst.path().join("video.en.vtt").exists());
    }
}

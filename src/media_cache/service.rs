use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use futures::StreamExt;
use mpvipc_async::{Mpv, MpvExt};
use tokio::sync::{Notify, mpsc};

use super::{
    downloader::{DownloadJob, DownloadOutcome, download, evict},
    key::canonical_key,
    m3u,
    probe::{
        FlatEntry, ProbeOutcome, StreamedEntry, VideoInfo, YtDlpEnv, probe, spawn_stream_probe,
    },
    store::MediaCacheStore,
};

const COORDINATOR_OBSERVE_ID: u64 = 1 << 40;
const COORDINATOR_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Per-URL probe knowledge; derived working state, never a serving surface.
#[derive(Debug, Clone)]
pub enum UrlState {
    Probing,
    Passthrough,
    Single(VideoInfo),
}

#[derive(Debug)]
pub struct MediaCacheService {
    store: MediaCacheStore,
    env: YtDlpEnv,
    min_free_space: u64,
    max_playlist_entries: usize,
    url_states: Mutex<HashMap<String, UrlState>>,
    /// URLs waiting to be resolved by the add worker, in add order.
    pending_adds: Mutex<VecDeque<String>>,
    adds_ready: Notify,
    /// Serializes our own playlist mutations so they can't interleave.
    playlist_mutations: tokio::sync::Mutex<()>,
}

impl MediaCacheService {
    /// Returns `None` when disabled, or on startup failure (greg-ng continues without caching).
    pub async fn start_from_config(
        config: &crate::config::MediaCacheConfig,
        cookies: &Path,
        mpv: Mpv,
    ) -> Option<Arc<Self>> {
        if !config.enabled {
            return None;
        }

        let cache_dir = config
            .directory
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(super::default_cache_path);
        let tmp_dir = config
            .tmp_directory
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(super::default_tmp_path);

        let store = match MediaCacheStore::open(&cache_dir, &tmp_dir) {
            Ok(store) => store,
            Err(err) => {
                tracing::error!("Failed to start media cache, continuing without it: {err:#}");
                return None;
            }
        };
        match tokio::process::Command::new(&config.ytdlp_path)
            .arg("--version")
            .output()
            .await
        {
            Ok(output) if output.status.success() => {}
            _ => tracing::warn!(
                "yt-dlp at {:?} is not runnable, nothing will be cached",
                config.ytdlp_path
            ),
        }
        tracing::info!(
            "Media cache enabled at {} (max {} bytes)",
            cache_dir.display(),
            config.max_size
        );

        let service = Arc::new(Self {
            store,
            env: YtDlpEnv {
                binary: config.ytdlp_path.clone(),
                cookies: if cookies.exists() {
                    Some(cookies.to_path_buf())
                } else {
                    None
                },
                format: config.format.clone(),
                sub_langs: config.sub_langs.clone(),
                stall_timeout: Duration::from_secs(config.stall_timeout_secs),
                max_size: config.max_size,
            },
            min_free_space: config.min_free_space,
            max_playlist_entries: config.max_playlist_entries,
            url_states: Mutex::new(HashMap::new()),
            pending_adds: Mutex::new(VecDeque::new()),
            adds_ready: Notify::new(),
            playlist_mutations: tokio::sync::Mutex::new(()),
        });

        let adder = service.clone();
        let mpv_adder = mpv.clone();
        tokio::spawn(async move {
            adder.run_add_worker(mpv_adder).await;
        });

        let coordinator = service.clone();
        tokio::spawn(async move {
            coordinator.run_coordinator(mpv).await;
        });

        Some(service)
    }

    pub fn spawn_health_check_thread(
        service: Arc<Self>,
        mut requests: mpsc::Receiver<crate::util::HealthCheckRequest>,
    ) {
        tokio::spawn(async move {
            while let Some(reply_tx) = requests.recv().await {
                let result = if service.is_healthy() {
                    Ok(())
                } else {
                    Err("media cache directory is not writable".to_string())
                };
                let _ = reply_tx.send(result);
            }
        });
    }

    pub fn is_healthy(&self) -> bool {
        std::fs::write(self.store.root().join(".healthcheck"), b"").is_ok()
    }

    /// Cache hits append the local file; everything else is resolved by the
    /// add worker before it enters the playlist — already titled, or
    /// expanded for playlists — so mpv never loads a raw unclassified URL.
    pub async fn resolve_and_load(self: &Arc<Self>, mpv: Mpv, url: String) -> anyhow::Result<()> {
        if !is_http_url(&url) {
            return append(&mpv, &url).await;
        }

        if let Some(entry) = self.store.lookup_url(&url) {
            self.store.record_use(&entry.meta.key);
            let item = m3u::Entry {
                filename: entry.media_path().to_string_lossy().into_owned(),
                title: entry.meta.title.clone(),
            };
            self.insert_entries(&mpv, None, std::slice::from_ref(&item))
                .await?;
            return Ok(());
        }

        let state = self.url_states.lock().unwrap().get(&url).cloned();
        match state {
            // Already being resolved; it is appended once, when ready.
            Some(UrlState::Probing) => {}
            Some(UrlState::Passthrough) => return append(&mpv, &url).await,
            // Known cacheable: re-add titled without re-probing.
            Some(UrlState::Single(info)) => {
                let item = m3u::Entry {
                    filename: url.clone(),
                    title: info.title.clone(),
                };
                self.insert_entries(&mpv, None, std::slice::from_ref(&item))
                    .await?;
            }
            None => {
                self.set_state(&url, UrlState::Probing);
                self.pending_adds.lock().unwrap().push_back(url);
                self.adds_ready.notify_one();
            }
        }

        Ok(())
    }

    /// Resolves pending adds in order: probes each URL and appends it to
    /// the playlist as entries stream in from yt-dlp.
    async fn run_add_worker(self: Arc<Self>, mpv: Mpv) {
        loop {
            let next = self.pending_adds.lock().unwrap().pop_front();
            let url = match next {
                Some(url) => url,
                None => {
                    let notified = self.adds_ready.notified();
                    if self.pending_adds.lock().unwrap().front().is_some() {
                        continue;
                    }
                    notified.await;
                    continue;
                }
            };
            tracing::debug!("Resolving added URL {url}");
            self.run_add(&mpv, &url).await;
        }
    }

    async fn run_add(self: &Arc<Self>, mpv: &Mpv, url: &str) {
        let mut entries = spawn_stream_probe(&self.env, url);
        let mut inserted = 0usize;
        let mut first: Option<StreamedEntry> = None;
        let mut buffer: Vec<m3u::Entry> = Vec::new();

        let mut cap = self.max_playlist_entries;
        while let Some(entry) = entries.recv().await {
            if inserted == 0 {
                first = Some(entry.clone());
                // Trust a reported playlist total; the configured cap only
                // bounds sources that report none (e.g. endless mixes).
                cap = entry
                    .playlist_count
                    .map_or(self.max_playlist_entries, |total| total as usize);
            }
            if inserted >= cap {
                tracing::debug!("Capping {url} at {cap} entries");
                break;
            }
            if inserted == 0 {
                first = Some(entry.clone());
            }
            self.register_child(
                &entry.url,
                entry.id.clone(),
                entry.extractor.clone(),
                entry.title.clone(),
            );
            buffer.push(m3u::Entry {
                filename: entry.url,
                title: entry.title,
            });
            inserted += 1;
            // The first entry lands immediately; the rest in batches.
            if buffer.len() >= 25 || inserted == 1 {
                if let Err(err) = self.insert_entries(mpv, None, &buffer).await {
                    tracing::warn!("Failed to add entries for {url}: {err}");
                }
                buffer.clear();
            }
        }
        if !buffer.is_empty()
            && let Err(err) = self.insert_entries(mpv, None, &buffer).await
        {
            tracing::warn!("Failed to add entries for {url}: {err}");
        }

        match (inserted, first) {
            // Nothing arrived: not cacheable, or the probe failed outright.
            (0, _) => {
                tracing::debug!("Not cacheable, adding as stream: {url}");
                self.set_state(url, UrlState::Passthrough);
                if let Err(err) = append(mpv, url).await {
                    tracing::warn!("Failed to add {url}: {err}");
                }
            }
            // A single entry that cannot be cached keeps its title but is
            // never downloaded.
            (1, Some(entry))
                if entry.duration.is_none()
                    || matches!(
                        entry.live_status.as_deref(),
                        Some("is_live") | Some("post_live") | Some("in_progress")
                    ) =>
            {
                self.set_state(url, UrlState::Passthrough);
            }
            (1, Some(entry)) if entry.id.is_some() => {}
            // Without an extractor id there is nothing to key a download on.
            (1, Some(_)) => self.set_state(url, UrlState::Passthrough),
            // The playlist URL itself is never playable; only its entries are.
            _ => self.set_state(url, UrlState::Passthrough),
        }
    }

    fn mark_probing(&self, url: &str) -> bool {
        let mut states = self.url_states.lock().unwrap();
        if matches!(
            states.get(url),
            Some(UrlState::Probing) | Some(UrlState::Passthrough)
        ) {
            return false;
        }
        states.insert(url.to_string(), UrlState::Probing);
        true
    }

    async fn process_probe_outcome(self: &Arc<Self>, mpv: &Mpv, url: &str, outcome: ProbeOutcome) {
        match outcome {
            ProbeOutcome::Passthrough => {
                tracing::debug!("Not cacheable, leaving as stream: {url}");
                self.set_state(url, UrlState::Passthrough);
            }
            ProbeOutcome::Single(info) => {
                self.set_state(url, UrlState::Single(info.clone()));
                let key = canonical_key(&info.extractor, &info.id);
                if let Some(existing) = self.store.lookup_key(&key) {
                    self.store.add_url_alias(&key, url);
                    self.store.record_use(&key);
                    let replacement = m3u::Entry {
                        filename: existing.media_path().to_string_lossy().into_owned(),
                        title: existing.meta.title.clone(),
                    };
                    swap_in(self, mpv, url, &replacement).await;
                } else {
                    self.fill_title(mpv, url).await;
                }
            }
            ProbeOutcome::Playlist { entries, total } => {
                self.set_state(url, UrlState::Passthrough);
                let cap = total.map_or(self.max_playlist_entries, |total| total as usize);
                let capped: Vec<FlatEntry> = entries.into_iter().take(cap).collect();
                tracing::debug!("Expanding playlist {url} into {} entries", capped.len());

                let mut items = Vec::with_capacity(capped.len());
                for entry in &capped {
                    self.register_child(
                        &entry.url,
                        entry.id.clone(),
                        entry.ie_key.clone(),
                        entry.title.clone(),
                    );
                    match self.store.lookup_url(&entry.url) {
                        Some(cached) => {
                            self.store.record_use(&cached.meta.key);
                            items.push(m3u::Entry {
                                filename: cached.media_path().to_string_lossy().into_owned(),
                                title: cached.meta.title.clone(),
                            });
                        }
                        None => items.push(m3u::Entry {
                            filename: entry.url.clone(),
                            title: entry.title.clone(),
                        }),
                    }
                }

                if let Err(err) = expand_entry(self, mpv, url, &items).await {
                    tracing::warn!("Failed to expand playlist {url}: {err}");
                }
            }
        }
    }

    /// Registers a playlist child with its identity, so the coordinator
    /// can download it without re-probing.
    fn register_child(
        &self,
        url: &str,
        id: Option<String>,
        extractor: Option<String>,
        title: Option<String>,
    ) {
        if let (Some(extractor), Some(id)) = (extractor, id) {
            self.set_state(
                url,
                UrlState::Single(VideoInfo {
                    extractor: extractor.to_lowercase(),
                    id,
                    title,
                    webpage_url: Some(url.to_string()),
                }),
            );
        }
    }

    fn set_state(&self, url: &str, state: UrlState) {
        const MAX_URL_STATES: usize = 10_000;

        let mut states = self.url_states.lock().unwrap();
        if states.len() >= MAX_URL_STATES {
            states.retain(|_, existing| !matches!(existing, UrlState::Passthrough));
        }
        states.insert(url.to_string(), state);
    }

    /// Title known to the cache for a filename, if any.
    fn known_title(&self, filename: &str) -> Option<String> {
        if let Some(state) = self.url_states.lock().unwrap().get(filename)
            && let UrlState::Single(info) = state
            && let Some(title) = &info.title
        {
            return Some(title.clone());
        }
        self.store
            .lookup_path(Path::new(filename))
            .and_then(|entry| entry.meta.title.clone())
    }

    /// Sets a known title on a playlist entry that lacks one, so every
    /// client reads it from mpv instead of from greg-ng.
    async fn fill_title(self: &Arc<Self>, mpv: &Mpv, filename: &str) {
        let Some(title) = self.known_title(filename) else {
            return;
        };
        let Ok(playlist) = mpv.get_playlist().await else {
            return;
        };
        if !playlist
            .0
            .iter()
            .any(|entry| !entry.current && entry.filename == filename && entry.title.is_none())
        {
            return;
        }

        let replacement = m3u::Entry {
            filename: filename.to_string(),
            title: Some(title),
        };
        if let Err(err) =
            expand_entry(self, mpv, filename, std::slice::from_ref(&replacement)).await
        {
            tracing::warn!("Failed to set title for {filename}: {err}");
        }
    }

    /// Prefetches the first uncached cacheable entry at/after the playing one; never cancels in-flight downloads.
    async fn run_coordinator(self: Arc<Self>, mpv: Mpv) {
        let _ = mpv
            .observe_property(COORDINATOR_OBSERVE_ID, "playlist")
            .await;
        let _ = mpv
            .observe_property(COORDINATOR_OBSERVE_ID, "playlist-pos")
            .await;
        let mut events = mpv.get_event_stream().await;

        self.evict_over_budget(&mpv).await;

        // Playing entries must keep looking recently used, or the evictor
        // would favor entries nobody plays.
        let mut last_use_recorded: Option<String> = None;
        // The entry mpv is loading may be running yt-dlp in its ytdl hook;
        // downloading the same URL concurrently makes that extraction fail,
        // so only download the current entry once it has finished loading.
        let mut current_loaded = false;

        loop {
            let playlist = match mpv.get_playlist().await {
                Ok(playlist) => playlist.0,
                Err(err) => {
                    tracing::warn!("Coordinator failed to read playlist: {err}");
                    Vec::new()
                }
            };

            if let Some(entry) = playlist.iter().find(|entry| entry.current)
                && let Some(cached) = self.store.lookup_path(Path::new(&entry.filename))
                && last_use_recorded.as_deref() != Some(&cached.meta.key)
            {
                self.store.record_use(&cached.meta.key);
                last_use_recorded = Some(cached.meta.key.clone());
            }

            self.heal_playlist(&mpv, &playlist).await;

            match self.plan_next_action(&playlist, current_loaded) {
                CoordinatorAction::Idle => {}
                CoordinatorAction::Probe(url) => {
                    if self.mark_probing(&url) {
                        let outcome = probe(&self.env, &url).await;
                        self.process_probe_outcome(&mpv, &url, outcome).await;
                    }
                }
                CoordinatorAction::Download(url, info) => {
                    self.run_download(&mpv, &url, info).await;
                }
            }

            match tokio::time::timeout(COORDINATOR_POLL_INTERVAL, events.next()).await {
                Err(_) => continue,
                Ok(None) => {
                    tracing::debug!("mpv event stream ended, stopping coordinator");
                    break;
                }
                Ok(Some(Ok(event))) => {
                    if !is_relevant_event(&event) {
                        continue;
                    }
                    match &event {
                        mpvipc_async::Event::FileLoaded => current_loaded = true,
                        mpvipc_async::Event::StartFile { .. }
                        | mpvipc_async::Event::EndFile { .. }
                        | mpvipc_async::Event::Idle => current_loaded = false,
                        mpvipc_async::Event::PropertyChange { name, .. }
                            if name == "playlist-pos" =>
                        {
                            current_loaded = false;
                        }
                        _ => {}
                    }
                }
                Ok(Some(Err(_))) => continue,
            }
        }
    }

    /// Rewrites playlist entries that lag behind the cache: URLs whose
    /// media is already cached, and entries missing a title we know.
    async fn heal_playlist(self: &Arc<Self>, mpv: &Mpv, playlist: &[mpvipc_async::PlaylistEntry]) {
        for entry in playlist.iter() {
            if entry.current {
                continue;
            }
            if is_http_url(&entry.filename)
                && let Some(cached) = self.store.lookup_url(&entry.filename)
            {
                self.store.record_use(&cached.meta.key);
                let replacement = m3u::Entry {
                    filename: cached.media_path().to_string_lossy().into_owned(),
                    title: cached.meta.title.clone(),
                };
                swap_in(self, mpv, &entry.filename, &replacement).await;
            } else if entry.title.is_none() {
                self.fill_title(mpv, &entry.filename).await;
            }
        }
    }

    fn plan_next_action(
        &self,
        playlist: &[mpvipc_async::PlaylistEntry],
        current_loaded: bool,
    ) -> CoordinatorAction {
        let start = playlist.iter().position(|entry| entry.current).unwrap_or(0);

        for (index, entry) in playlist.iter().enumerate().skip(start) {
            // Never race mpv's ytdl hook for an entry that is still loading.
            if index == start && entry.current && !current_loaded {
                continue;
            }
            if !is_http_url(&entry.filename) {
                continue;
            }
            if self.store.lookup_url(&entry.filename).is_some() {
                continue;
            }
            match self
                .url_states
                .lock()
                .unwrap()
                .get(&entry.filename)
                .cloned()
            {
                Some(UrlState::Probing) | Some(UrlState::Passthrough) => continue,
                Some(UrlState::Single(info)) => {
                    return CoordinatorAction::Download(entry.filename.clone(), info);
                }
                None => return CoordinatorAction::Probe(entry.filename.clone()),
            }
        }

        CoordinatorAction::Idle
    }

    async fn run_download(self: &Arc<Self>, mpv: &Mpv, url: &str, info: VideoInfo) {
        tracing::debug!("Prefetching {url}");
        let job = DownloadJob {
            url: url.to_string(),
            info,
        };
        match download(&self.env, &self.store, job).await {
            DownloadOutcome::Cached(entry) => {
                let replacement = m3u::Entry {
                    filename: entry.media_path().to_string_lossy().into_owned(),
                    title: entry.meta.title.clone(),
                };
                for alias in entry.meta.urls.clone() {
                    swap_in(self, mpv, &alias, &replacement).await;
                }
                self.evict_over_budget(mpv).await;
            }
            DownloadOutcome::Skipped(reason) => {
                tracing::info!("Not caching {url}: {reason}");
                self.set_state(url, UrlState::Passthrough);
            }
        }
    }

    async fn evict_over_budget(self: &Arc<Self>, mpv: &Mpv) {
        let pinned = self.pinned_paths(mpv).await;
        evict(&self.store, self.env.max_size, self.min_free_space, &pinned);
    }

    async fn pinned_paths(&self, mpv: &Mpv) -> HashSet<PathBuf> {
        match mpv.get_playlist().await {
            Ok(playlist) => playlist
                .0
                .iter()
                .filter(|entry| {
                    entry
                        .filename
                        .starts_with(self.store.root().to_string_lossy().as_ref())
                })
                .filter_map(|entry| self.store.lookup_path(Path::new(&entry.filename)))
                .map(|entry| entry.media_path())
                .collect(),
            Err(_) => HashSet::new(),
        }
    }

    /// Inserts entries via a temp M3U playlist so mpv's own parser
    /// attaches the titles; `index` of `None` appends. The lone raw mpv
    /// command: belongs in the interface library once it grows a loadlist
    /// index argument.
    async fn insert_entries(
        &self,
        mpv: &Mpv,
        index: Option<usize>,
        entries: &[m3u::Entry],
    ) -> anyhow::Result<()> {
        let file = m3u::write(self.store.tmp_root(), entries)?;
        let path = file.path().to_string();
        match index {
            Some(index) => {
                mpv.run_command_raw("loadlist", &[&path, "insert-at", &index.to_string()])
                    .await
            }
            None => {
                mpv.run_command_raw("loadlist", &[&path, "append", "-1"])
                    .await
            }
        }?;
        Ok(())
    }
}

enum CoordinatorAction {
    Idle,
    Probe(String),
    Download(String, VideoInfo),
}

fn is_relevant_event(event: &mpvipc_async::Event) -> bool {
    match event {
        mpvipc_async::Event::PropertyChange { name, .. } => {
            name == "playlist" || name == "playlist-pos"
        }
        mpvipc_async::Event::StartFile { .. }
        | mpvipc_async::Event::FileLoaded
        | mpvipc_async::Event::EndFile { .. }
        | mpvipc_async::Event::Idle => true,
        _ => false,
    }
}

async fn append(mpv: &Mpv, path: &str) -> anyhow::Result<()> {
    mpv.playlist_add(
        path,
        mpvipc_async::PlaylistAddTypeOptions::File,
        mpvipc_async::PlaylistAddOptions::Append,
    )
    .await?;
    Ok(())
}

/// Replaces the first non-playing entry matching `old` with a cached file.
async fn swap_in(service: &MediaCacheService, mpv: &Mpv, old: &str, cached: &m3u::Entry) {
    if let Err(err) = expand_entry(service, mpv, old, std::slice::from_ref(cached)).await {
        tracing::warn!("Failed to swap {old} to cached file: {err}");
    }
}

/// Replaces the first non-playing match with `entries` at its position.
/// An empty `entries` list keeps the original so mpv can still stream it.
async fn expand_entry(
    service: &MediaCacheService,
    mpv: &Mpv,
    old_filename: &str,
    entries: &[m3u::Entry],
) -> anyhow::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }

    let _guard = service.playlist_mutations.lock().await;
    let playlist = mpv.get_playlist().await?;
    let Some(index) = find_entry(&playlist.0, old_filename) else {
        return Ok(());
    };

    service.insert_entries(mpv, Some(index), entries).await?;

    // Matches by content, not index, to survive concurrent playlist edits.
    let playlist = mpv.get_playlist().await?;
    let Some(old_index) = find_entry(&playlist.0, old_filename) else {
        return Ok(());
    };
    mpv.playlist_remove_id(old_index)
        .await
        .map_err(|e| e.into())
}

fn find_entry(entries: &[mpvipc_async::PlaylistEntry], filename: &str) -> Option<usize> {
    entries
        .iter()
        .position(|entry| !entry.current && entry.filename == filename)
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_urls_are_cacheable() {
        assert!(is_http_url("http://example.com/a"));
        assert!(is_http_url("https://youtu.be/x"));
        assert!(!is_http_url("/mnt/local.mp3"));
        assert!(!is_http_url("file:///tmp/a.mkv"));
        assert!(!is_http_url("rtsp://stream"));
    }
}

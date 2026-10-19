use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::Context;

use super::{
    lock::{DirLock, LOCK_FILE_NAME, read_lock_info_at},
    meta::{MetaEntry, SCHEMA_VERSION},
};

const META_FILE_NAME: &str = "meta.json";
const INSTANCE_LOCK_FILE_NAME: &str = "instance.lock";

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub dir: PathBuf,
    pub meta: MetaEntry,
}

impl CacheEntry {
    pub fn media_path(&self) -> PathBuf {
        self.dir.join(&self.meta.media_file)
    }
}

/// In-memory index over the on-disk cache; entry directories are the source of truth.
#[derive(Debug)]
pub struct MediaCacheStore {
    root: PathBuf,
    tmp_root: PathBuf,
    entries: RwLock<HashMap<String, Arc<CacheEntry>>>,
    urls: RwLock<HashMap<String, Arc<CacheEntry>>>,
    content: RwLock<HashMap<String, Arc<CacheEntry>>>,
    _instance_lock: DirLock,
}

impl MediaCacheStore {
    pub fn open(root: &Path, tmp_root: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(root)
            .with_context(|| format!("Failed to create cache directory {}", root.display()))?;
        fs::create_dir_all(tmp_root)
            .with_context(|| format!("Failed to create tmp directory {}", tmp_root.display()))?;

        let instance_lock = DirLock::try_acquire(&root.join(INSTANCE_LOCK_FILE_NAME))
            .with_context(|| format!("Failed to lock cache directory {}", root.display()))?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Cache directory {} is locked by another greg-ng instance",
                    root.display()
                )
            })?;

        let store = Self {
            root: root.to_path_buf(),
            tmp_root: tmp_root.to_path_buf(),
            entries: RwLock::new(HashMap::new()),
            urls: RwLock::new(HashMap::new()),
            content: RwLock::new(HashMap::new()),
            _instance_lock: instance_lock,
        };

        store.wipe_tmp()?;
        store.scan_and_reconcile()?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn tmp_root(&self) -> &Path {
        &self.tmp_root
    }

    pub fn entry_dir(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }

    pub fn lookup_key(&self, key: &str) -> Option<Arc<CacheEntry>> {
        self.entries.read().unwrap().get(key).cloned()
    }

    pub fn lookup_url(&self, url: &str) -> Option<Arc<CacheEntry>> {
        self.urls.read().unwrap().get(url).cloned()
    }

    pub fn lookup_path(&self, path: &Path) -> Option<Arc<CacheEntry>> {
        self.entries
            .read()
            .unwrap()
            .values()
            .find(|entry| entry.media_path() == path)
            .cloned()
    }

    pub fn lookup_content(&self, hash: &str) -> Option<Arc<CacheEntry>> {
        self.content.read().unwrap().get(hash).cloned()
    }

    pub fn insert(&self, entry: CacheEntry) -> anyhow::Result<Arc<CacheEntry>> {
        let arc = Arc::new(entry);
        self.persist(&arc)?;
        self.index(arc.clone());
        Ok(arc)
    }

    pub fn record_use(&self, key: &str) -> Option<Arc<CacheEntry>> {
        self.mutate(key, |meta| meta.record_use(super::unix_now()))
    }

    pub fn add_url_alias(&self, key: &str, url: &str) -> Option<Arc<CacheEntry>> {
        self.mutate(key, |meta| {
            if !meta.urls.iter().any(|u| u == url) {
                meta.urls.push(url.to_string());
            }
        })
    }

    pub fn total_size(&self) -> u64 {
        self.entries
            .read()
            .unwrap()
            .values()
            .map(|entry| entry.meta.size)
            .sum()
    }

    pub fn entries_by_last_used(&self) -> Vec<Arc<CacheEntry>> {
        let mut entries: Vec<Arc<CacheEntry>> =
            self.entries.read().unwrap().values().cloned().collect();
        entries.sort_by_key(|entry| entry.meta.last_used);
        entries
    }

    /// Re-points a shared content hash at another entry with the same content.
    pub fn remove(&self, key: &str) -> Option<Arc<CacheEntry>> {
        let entry = self.entries.write().unwrap().remove(key)?;

        {
            let mut urls = self.urls.write().unwrap();
            urls.retain(|_, value| value.meta.key != key);
        }

        if let Some(hash) = entry.meta.content_hash.as_deref() {
            let replacement = self
                .entries
                .read()
                .unwrap()
                .values()
                .find(|other| other.meta.content_hash.as_deref() == Some(hash))
                .cloned();
            let mut content = self.content.write().unwrap();
            match replacement {
                Some(other) => {
                    content.insert(hash.to_string(), other);
                }
                None => {
                    content.remove(hash);
                }
            }
        }

        let lock = DirLock::try_acquire(&entry.dir.join(LOCK_FILE_NAME))
            .ok()
            .flatten();
        if let Err(err) = fs::remove_dir_all(&entry.dir) {
            tracing::warn!(
                "Failed to remove cache entry {}: {err}",
                entry.dir.display()
            );
        }
        drop(lock);

        Some(entry)
    }

    fn mutate(&self, key: &str, change: impl FnOnce(&mut MetaEntry)) -> Option<Arc<CacheEntry>> {
        // Hold the lock across clone-persist-replace so concurrent mutates can't clobber each other.
        let updated = {
            let mut entries = self.entries.write().unwrap();
            let entry = entries.get(key)?;
            let mut meta = entry.meta.clone();
            change(&mut meta);
            let updated = Arc::new(CacheEntry {
                dir: entry.dir.clone(),
                meta,
            });

            if let Err(err) = self.persist(&updated) {
                tracing::warn!("Failed to update entry {key}: {err}");
                return None;
            }
            entries.insert(key.to_string(), updated.clone());
            updated
        };

        self.reindex_urls(key, &updated);
        Some(updated)
    }

    fn index(&self, entry: Arc<CacheEntry>) {
        self.entries
            .write()
            .unwrap()
            .insert(entry.meta.key.clone(), entry.clone());
        self.reindex_urls(&entry.meta.key, &entry);

        if let Some(hash) = entry.meta.content_hash.as_deref() {
            let mut content = self.content.write().unwrap();
            match content.get(hash) {
                Some(existing) if existing.meta.created_at <= entry.meta.created_at => {}
                _ => {
                    content.insert(hash.to_string(), entry);
                }
            }
        }
    }

    fn reindex_urls(&self, key: &str, entry: &Arc<CacheEntry>) {
        let mut urls = self.urls.write().unwrap();
        for url in &entry.meta.urls {
            urls.insert(url.clone(), entry.clone());
        }
        urls.retain(|url, value| {
            value.meta.key != key || entry.meta.urls.iter().any(|candidate| candidate == url)
        });
    }

    fn persist(&self, entry: &Arc<CacheEntry>) -> anyhow::Result<()> {
        fs::create_dir_all(&entry.dir)
            .with_context(|| format!("Failed to create entry directory {}", entry.dir.display()))?;

        let _lock = DirLock::try_acquire(&entry.dir.join(LOCK_FILE_NAME))
            .with_context(|| format!("Failed to lock entry {}", entry.dir.display()))?
            .ok_or_else(|| {
                anyhow::anyhow!("Entry {} is locked by another process", entry.dir.display())
            })?;

        let tmp = entry.dir.join(format!("{META_FILE_NAME}.tmp"));
        let final_path = entry.dir.join(META_FILE_NAME);
        fs::write(&tmp, serde_json::to_vec_pretty(&entry.meta)?)?;
        fs::rename(&tmp, &final_path)
            .with_context(|| format!("Failed to persist {}", final_path.display()))?;
        Ok(())
    }

    fn wipe_tmp(&self) -> anyhow::Result<()> {
        if let Err(err) = fs::remove_dir_all(&self.tmp_root)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                "Failed to wipe tmp directory {}: {err}",
                self.tmp_root.display()
            );
        }
        fs::create_dir_all(&self.tmp_root).with_context(|| {
            format!("Failed to create tmp directory {}", self.tmp_root.display())
        })?;
        Ok(())
    }

    fn scan_and_reconcile(&self) -> anyhow::Result<()> {
        let tmp_canonical = self.tmp_root.canonicalize().ok();
        let mut found = Vec::new();

        for child in fs::read_dir(&self.root)
            .with_context(|| format!("Failed to read cache directory {}", self.root.display()))?
            .flatten()
        {
            let path = child.path();
            if !path.is_dir() {
                continue;
            }
            if tmp_canonical.as_ref() == path.canonicalize().ok().as_ref() {
                continue;
            }

            match self.load_entry(&path) {
                Ok(Some(entry)) => found.push(Arc::new(entry)),
                Ok(None) => {}
                Err(reason) => {
                    tracing::warn!("Removing orphaned cache entry {}: {reason}", path.display());
                    if let Err(err) = fs::remove_dir_all(&path) {
                        tracing::warn!("Failed to remove {}: {err}", path.display());
                    }
                }
            }
        }

        for entry in found {
            tracing::debug!(
                "Cached entry {} ({})",
                entry.meta.key,
                entry.meta.title.as_deref().unwrap_or("?")
            );
            self.index(entry);
        }
        Ok(())
    }

    fn load_entry(&self, dir: &Path) -> anyhow::Result<Option<CacheEntry>> {
        let meta_path = dir.join(META_FILE_NAME);
        if !meta_path.exists() {
            return Err(anyhow::anyhow!("missing {META_FILE_NAME}"));
        }

        let meta: MetaEntry = serde_json::from_str(
            &fs::read_to_string(&meta_path)
                .with_context(|| format!("Failed to read {}", meta_path.display()))?,
        )
        .with_context(|| format!("Failed to parse {}", meta_path.display()))?;

        if meta.schema_version != SCHEMA_VERSION {
            return Err(anyhow::anyhow!(
                "unsupported schema version {}",
                meta.schema_version
            ));
        }

        let key = dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid directory name"))?;
        if key != meta.key {
            return Err(anyhow::anyhow!("directory/meta key mismatch"));
        }

        if !dir.join(&meta.media_file).exists() {
            return Err(anyhow::anyhow!(
                "media file {:?} is missing",
                meta.media_file
            ));
        }

        if let Some(info) = read_lock_info_at(&dir.join(LOCK_FILE_NAME))
            && info.expires_at <= super::unix_now()
        {
            tracing::debug!("Removing stale lock in {}", dir.display());
            let _ = fs::remove_file(dir.join(LOCK_FILE_NAME));
        }

        Ok(Some(CacheEntry {
            dir: dir.to_path_buf(),
            meta,
        }))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::media_cache::meta::SCHEMA_VERSION;

    pub(crate) fn sample_entry(key: &str, urls: &[&str], created_at: u64) -> CacheEntry {
        CacheEntry {
            dir: PathBuf::from(key),
            meta: MetaEntry {
                schema_version: SCHEMA_VERSION,
                key: key.to_string(),
                urls: urls.iter().map(|u| u.to_string()).collect(),
                title: Some(format!("Entry {key}")),
                media_file: "video.mkv".to_string(),
                size: 1000,
                content_hash: Some(format!("hash-{key}")),
                created_at,
                last_used: created_at,
            },
        }
    }

    fn setup() -> (tempfile::TempDir, MediaCacheStore) {
        let root = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store = MediaCacheStore::open(root.path(), tmp.path()).unwrap();
        (root, store)
    }

    fn insert_on_disk(root: &Path, entry: &CacheEntry) {
        let dir = root.join(&entry.meta.key);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("video.mkv"), b"data").unwrap();
        fs::write(
            dir.join("meta.json"),
            serde_json::to_vec_pretty(&entry.meta).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn open_scans_existing_entries() {
        let root = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        insert_on_disk(
            root.path(),
            &sample_entry("aaaa", &["https://x/1", "https://x/2"], 100),
        );

        let store = MediaCacheStore::open(root.path(), tmp.path()).unwrap();
        let entry = store.lookup_url("https://x/2").expect("url alias indexed");
        assert_eq!(entry.meta.key, "aaaa");
        assert_eq!(store.total_size(), 1000);
    }

    #[test]
    fn open_removes_orphans() {
        let root = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();

        let orphan = root.path().join("bbbb");
        fs::create_dir_all(&orphan).unwrap();
        fs::write(orphan.join("video.mkv"), b"data").unwrap();

        let broken = root.path().join("cccc");
        insert_on_disk(root.path(), &sample_entry("cccc", &["https://x/3"], 100));
        fs::remove_file(broken.join("video.mkv")).unwrap();

        let store = MediaCacheStore::open(root.path(), tmp.path()).unwrap();
        assert!(store.lookup_key("bbbb").is_none());
        assert!(store.lookup_key("cccc").is_none());
        assert!(!orphan.exists());
        assert!(!broken.exists());
    }

    #[test]
    fn insert_persists_and_indexes() {
        let (root, store) = setup();
        let mut entry = sample_entry("dddd", &["https://x/4"], 100);
        entry.dir = store.entry_dir("dddd");

        store.insert(entry).unwrap();

        let found = store.lookup_url("https://x/4").expect("indexed");
        assert_eq!(found.meta.key, "dddd");
        assert!(store.root().join("dddd").join("meta.json").exists());
        fs::write(store.root().join("dddd").join("video.mkv"), b"data").unwrap();
        drop(store);

        let tmp = tempfile::tempdir().unwrap();
        let reopened = MediaCacheStore::open(root.path(), tmp.path()).unwrap();
        assert!(reopened.lookup_key("dddd").is_some());
    }

    #[test]
    fn record_use_updates_last_used() {
        let (_root, store) = setup();
        let mut entry = sample_entry("eeee", &["https://x/5"], 100);
        entry.dir = store.entry_dir("eeee");
        store.insert(entry).unwrap();

        let updated = store.record_use("eeee").expect("entry exists");
        assert!(updated.meta.last_used > 100);
        assert_eq!(store.entries_by_last_used()[0].meta.key, "eeee");
    }

    #[test]
    fn add_url_alias_is_idempotent() {
        let (_root, store) = setup();
        let mut entry = sample_entry("ffff", &["https://x/6"], 100);
        entry.dir = store.entry_dir("ffff");
        store.insert(entry).unwrap();

        store.add_url_alias("ffff", "https://x/7").unwrap();
        store.add_url_alias("ffff", "https://x/7").unwrap();

        let found = store.lookup_url("https://x/7").expect("alias indexed");
        assert_eq!(
            found
                .meta
                .urls
                .iter()
                .filter(|u| u.as_str() == "https://x/7")
                .count(),
            1
        );
    }

    #[test]
    fn content_index_prefers_older_entry() {
        let (_root, store) = setup();
        let mut old = sample_entry("gggg", &["https://x/8"], 100);
        old.meta.content_hash = Some("shared".to_string());
        old.dir = store.entry_dir("gggg");
        let mut new = sample_entry("hhhh", &["https://x/9"], 200);
        new.meta.content_hash = Some("shared".to_string());
        new.dir = store.entry_dir("hhhh");

        store.insert(old).unwrap();
        store.insert(new).unwrap();

        assert_eq!(store.lookup_content("shared").unwrap().meta.key, "gggg");
    }

    #[test]
    fn wipe_tmp_clears_and_recreates_directory() {
        let root = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let junk = tmp.path().join("junk");
        fs::write(&junk, b"x").unwrap();

        let store = MediaCacheStore::open(root.path(), tmp.path()).unwrap();
        assert!(!junk.exists());
        assert!(store.tmp_root().is_dir());
    }

    #[test]
    fn second_instance_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let _first = MediaCacheStore::open(root.path(), tmp.path()).unwrap();
        assert!(MediaCacheStore::open(root.path(), tmp.path()).is_err());
    }

    #[test]
    fn remove_clears_indexes_and_repoints_content() {
        let (_root, store) = setup();
        let mut old = sample_entry("iiii", &["https://x/10"], 100);
        old.meta.content_hash = Some("shared".to_string());
        old.dir = store.entry_dir("iiii");
        let mut young = sample_entry("jjjj", &["https://x/11"], 200);
        young.meta.content_hash = Some("shared".to_string());
        young.dir = store.entry_dir("jjjj");
        store.insert(old).unwrap();
        store.insert(young).unwrap();

        let removed = store.remove("iiii").expect("entry removed");
        assert!(removed.media_path().starts_with(store.root()));
        assert!(!store.entry_dir("iiii").exists());
        assert!(store.lookup_key("iiii").is_none());
        assert!(store.lookup_url("https://x/10").is_none());
        assert_eq!(store.lookup_content("shared").unwrap().meta.key, "jjjj");

        store.remove("jjjj").unwrap();
        assert!(store.lookup_content("shared").is_none());
        assert_eq!(store.total_size(), 0);
    }
}

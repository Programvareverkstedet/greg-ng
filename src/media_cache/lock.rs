use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::Seek,
    path::Path,
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

const LOCK_TTL_SECS: u64 = 3600;

pub const LOCK_FILE_NAME: &str = "lock";

#[derive(Debug, Serialize, Deserialize)]
pub struct LockInfo {
    pub holder: String,
    pub acquired_at: u64,
    pub expires_at: u64,
}

/// Exclusive flock; the file keeps the last holder's info for stale detection.
pub struct DirLock {
    // Held only for its flock; dropping the file releases the lock.
    _file: File,
}

impl std::fmt::Debug for DirLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirLock").finish_non_exhaustive()
    }
}

impl DirLock {
    pub fn try_acquire(lock_path: &Path) -> anyhow::Result<Option<Self>> {
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create lock parent directory {}",
                    parent.display()
                )
            })?;
        }

        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .with_context(|| format!("Failed to open lock file {}", lock_path.display()))?;

        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(err)) => {
                return Err(err).with_context(|| format!("Failed to lock {}", lock_path.display()));
            }
        }

        let now = super::unix_now();
        let info = LockInfo {
            holder: holder_identity(),
            acquired_at: now,
            expires_at: now + LOCK_TTL_SECS,
        };
        if let Err(err) = write_lock_info(&file, &info) {
            tracing::warn!(
                "Failed to record lock info for {}: {err}",
                lock_path.display()
            );
        }

        Ok(Some(Self { _file: file }))
    }
}

pub fn read_lock_info_at(lock_path: &Path) -> Option<LockInfo> {
    let file = File::open(lock_path).ok()?;
    read_lock_info(&file)
}

fn read_lock_info(file: &File) -> Option<LockInfo> {
    serde_json::from_reader(file).ok()
}

fn write_lock_info(file: &File, info: &LockInfo) -> anyhow::Result<()> {
    let mut file = file;
    file.set_len(0).ok();
    file.seek(std::io::SeekFrom::Start(0))?;
    serde_json::to_writer(&mut file, info)?;
    file.sync_data().ok();
    file.seek(std::io::SeekFrom::Start(0))?;
    Ok(())
}

fn holder_identity() -> String {
    let host = fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    format!("greg-ng pid={} host={}", std::process::id(), host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join(LOCK_FILE_NAME)
    }

    #[test]
    fn locks_are_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let first = DirLock::try_acquire(&lock_path(&dir))
            .unwrap()
            .expect("first lock should be acquired");
        let second = DirLock::try_acquire(&lock_path(&dir)).unwrap();
        assert!(second.is_none());

        let info = read_lock_info_at(&lock_path(&dir)).expect("lock info should be recorded");
        assert!(info.holder.contains("greg-ng"));
        drop(first);

        DirLock::try_acquire(&lock_path(&dir))
            .unwrap()
            .expect("lock should be re-acquirable after drop");
    }

    #[test]
    fn lock_info_is_readable_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        {
            DirLock::try_acquire(&lock_path(&dir))
                .unwrap()
                .expect("lock should be acquired");
        }
        let info = read_lock_info_at(&lock_path(&dir)).expect("info should persist after unlock");
        assert!(info.expires_at > info.acquired_at);
    }
}

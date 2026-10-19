mod key;
mod lock;
mod meta;
mod store;

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn default_cache_path() -> PathBuf {
    if let Ok(cache_directory) = std::env::var("CACHE_DIRECTORY") {
        return PathBuf::from(cache_directory).join("media");
    }
    if let Ok(xdg_cache_home) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(xdg_cache_home).join("greg-ng").join("media");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".cache")
            .join("greg-ng")
            .join("media");
    }
    PathBuf::from("/var/cache/greg-ng/media")
}

fn default_tmp_path() -> PathBuf {
    std::env::temp_dir().join("greg-ng-media")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

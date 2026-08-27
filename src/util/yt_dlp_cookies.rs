use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

/// Current yt-dlp cookies state
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct YtDlpCookies {
    pub cookies: Option<String>,
    pub present: bool,
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct YtDlpCookiesState {
    path: Arc<PathBuf>,
    tx: watch::Sender<YtDlpCookies>,
}

impl YtDlpCookiesState {
    pub fn load(path: PathBuf) -> Self {
        let initial = match std::fs::read_to_string(&path) {
            Ok(cookies) => {
                let updated_at = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs());

                YtDlpCookies {
                    present: true,
                    updated_at,
                    cookies: Some(cookies),
                }
            }
            Err(_) => YtDlpCookies::default(),
        };

        let (tx, _) = watch::channel(initial);
        Self {
            path: Arc::new(path),
            tx,
        }
    }

    pub fn get(&self) -> YtDlpCookies {
        self.tx.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<YtDlpCookies> {
        self.tx.subscribe()
    }

    pub fn set(&self, cookies: String) -> anyhow::Result<YtDlpCookies> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&*self.path, &cookies)?;

        let updated_at = std::fs::metadata(&*self.path)
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| SystemTime::now())
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());

        let value = YtDlpCookies {
            present: true,
            updated_at,
            cookies: Some(cookies),
        };
        let _ = self.tx.send(value.clone());
        Ok(value)
    }
}

const EMPTY_NETSCAPE_COOKIE_FILE: &str = "\
# Netscape HTTP Cookie File
# http://curl.haxx.se/rfc/cookie_spec.html
# This is a generated file!  Do not edit.
";

pub fn ensure_cookies_file_exists(path: &std::path::Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !path.exists() {
        std::fs::write(path, EMPTY_NETSCAPE_COOKIE_FILE)?;
    }
    Ok(())
}

pub fn default_cookies_path() -> PathBuf {
    if let Some(cache_dir) = std::env::var_os("CACHE_DIRECTORY") {
        return PathBuf::from(cache_dir).join("yt-dlp-cookies.txt");
    }

    if let Some(xdg_cache) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(xdg_cache).join("greg-ng/yt-dlp-cookies.txt");
    }

    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache/greg-ng/yt-dlp-cookies.txt");
    }

    PathBuf::from("/var/cache/greg-ng/yt-dlp-cookies.txt")
}

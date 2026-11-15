use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

/// Current yt-dlp cookies state
///
/// This uses a Netscape cookie file internally (i.e. .netrc), as that is the format yt-dlp expects.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct YtDlpCookies {
    pub cookies: Option<String>,
    pub present: bool,
    pub updated_at: Option<u64>,
}

impl YtDlpCookies {
    fn read(path: &Path) -> std::io::Result<YtDlpCookies> {
        let cookies = std::fs::read_to_string(path)?;
        let modified = std::fs::metadata(path)?.modified()?;
        let updated_at = modified
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        tracing::info!(
            "Read yt-dlp cookies from {}: {}, last updated {}",
            path.display(),
            Self::summary(&cookies, now),
            humantime::format_rfc3339_seconds(modified),
        );

        Ok(YtDlpCookies {
            present: Self::has_cookies(&cookies),
            cookies: Some(cookies),
            updated_at,
        })
    }

    /// The cookie entries of a cookie file
    fn cookie_lines(cookies: &str) -> impl Iterator<Item = &str> {
        cookies
            .lines()
            .map(str::trim)
            // Lines starting with `#HttpOnly_` are cookies, not comments
            .filter(|line| {
                !line.is_empty() && (!line.starts_with('#') || line.starts_with("#HttpOnly_"))
            })
    }

    /// Whether the cookie file contains any cookies.
    fn has_cookies(cookies: &str) -> bool {
        Self::cookie_lines(cookies).next().is_some()
    }

    /// Summarizes the cookie file for logging
    fn summary(cookies: &str, now: u64) -> String {
        let (total, expired) = Self::cookie_lines(cookies)
            // 5th field is an expiry unix timestamp, 0 means a session cookie
            .map(|line| line.split('\t').nth(4).and_then(|e| e.parse::<u64>().ok()))
            .fold((0, 0), |(total, expired), expiry| {
                let is_expired = matches!(expiry, Some(e) if e != 0 && e < now);
                (total + 1, expired + usize::from(is_expired))
            });
        format!("{total} cookies ({expired} expired)")
    }
}

#[derive(Debug, Clone)]
pub struct YtDlpCookiesState {
    path: Arc<Path>,
    tx: watch::Sender<YtDlpCookies>,
}

impl YtDlpCookiesState {
    pub fn ensure_file_exists(path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if !path.exists() {
            std::fs::write(path, EMPTY_NETSCAPE_COOKIE_FILE)?;
        }
        Ok(())
    }

    pub fn default_path() -> PathBuf {
        let env = |var| std::env::var_os(var).map(PathBuf::from);
        env("CACHE_DIRECTORY")
            .or_else(|| env("XDG_CACHE_HOME").map(|p| p.join("greg-ng")))
            .or_else(|| env("HOME").map(|p| p.join(".cache/greg-ng")))
            .unwrap_or_else(|| PathBuf::from("/var/cache/greg-ng"))
            .join("yt-dlp-cookies.txt")
    }

    pub fn load(path: PathBuf) -> Self {
        let initial = YtDlpCookies::read(&path).unwrap_or_else(|e| {
            tracing::warn!("Failed to read yt-dlp cookies from {}: {e}", path.display());
            YtDlpCookies::default()
        });
        Self {
            path: path.into(),
            tx: watch::Sender::new(initial),
        }
    }

    pub fn get(&self) -> YtDlpCookies {
        self.tx.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<YtDlpCookies> {
        self.tx.subscribe()
    }

    pub fn set(&self, cookies: String) -> anyhow::Result<YtDlpCookies> {
        std::fs::write(&self.path, cookies)?;
        let value = YtDlpCookies::read(&self.path)?;
        self.tx.send_replace(value.clone());
        Ok(value)
    }
}

const EMPTY_NETSCAPE_COOKIE_FILE: &str = "\
# Netscape HTTP Cookie File
# http://curl.haxx.se/rfc/cookie_spec.html
# This is a generated file!  Do not edit.
";

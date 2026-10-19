use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Deserializer};
use tempfile::NamedTempFile;
use tracing::level_filters::LevelFilter;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub mpv: MpvConfig,
    pub media_cache: MediaCacheConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Hostname to bind the various APIs to.
    pub host: String,

    /// Port to bind the various APIs to.
    pub port: u16,

    #[serde(deserialize_with = "deserialize_level_filter")]
    pub verbosity: LevelFilter,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 8009,
            verbosity: LevelFilter::ERROR,
        }
    }
}

fn deserialize_level_filter<'de, D>(deserializer: D) -> Result<LevelFilter, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer)?
        .parse()
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct MpvConfig {
    /// Location of the mpv socket to connect to, when not auto-starting mpv.
    pub socket_path: Option<String>,

    /// Location of the mpv binary, used when auto-starting mpv.
    pub executable_path: Option<String>,

    /// An optional config file for mpv, used when auto-starting mpv.
    pub config_file: Option<String>,

    /// Instead of using the socket path, start a new private mpv instance
    /// with the given executable and config file.
    ///
    /// Defaults to `true` if `socket_path` is unset, false otherwise.
    pub auto_start: Option<bool>,

    /// A generated temporary file that contains mpv config.
    /// Keeping it here makes the temporary file bound to the lifetime of
    /// the configuration, so we won't have to worry too much about it in
    /// the rest of the code. It's not expected to actually be part of the
    /// parsed config file.
    #[serde(skip)]
    pub resolved_config_file: Option<NamedTempFile>,
}

impl MpvConfig {
    pub fn should_auto_start(&self) -> bool {
        self.auto_start
            .unwrap_or_else(|| self.socket_path.is_none())
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct MediaCacheConfig {
    /// Whether to cache media resolved with yt-dlp on local storage.
    pub enabled: bool,

    /// Directory holding the media cache; defaults to `$CACHE_DIRECTORY/media`.
    pub directory: Option<String>,

    /// Directory for in-progress downloads; defaults to `<tmp>/greg-ng-media`.
    pub tmp_directory: Option<String>,

    /// Maximum total size of cached media, e.g. `"10 GiB"`; `-1` means unlimited (bounded by `min_free_space`).
    #[serde(deserialize_with = "deserialize_max_size")]
    pub max_size: i64,

    /// Minimum free space to keep on the cache filesystem, e.g. `"1 GiB"`.
    #[serde(deserialize_with = "deserialize_min_free_space")]
    pub min_free_space: u64,

    /// yt-dlp format string used for downloads.
    pub format: String,

    /// Native subtitle languages to download, e.g. `"en.*,no.*"`.
    pub sub_langs: String,

    /// Path to the yt-dlp executable.
    pub ytdlp_path: String,

    /// Seconds without yt-dlp output or download growth before aborting.
    pub stall_timeout_secs: u64,

    /// Bounds playlist expansion for sources reporting no total (e.g. endless mixes); finite playlists expand fully.
    pub max_playlist_entries: usize,
}

impl Default for MediaCacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: None,
            tmp_directory: None,
            max_size: -1,
            min_free_space: 1024 * 1024 * 1024,
            format: "bestvideo[height<=?720][vcodec!~='vp0?9']+bestaudio/bestaudio/best"
                .to_string(),
            sub_langs: "en.*,no.*".to_string(),
            ytdlp_path: "yt-dlp".to_string(),
            stall_timeout_secs: 30,
            max_playlist_entries: 500,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SizeValue {
    Bytes(i64),
    Human(String),
}

fn parse_size(input: &str) -> anyhow::Result<u64> {
    let input = input.trim();
    let split = input
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(input.len());
    let (number, unit) = input.split_at(split);

    let multiplier = match unit.trim() {
        "" | "B" => 1,
        "KiB" => 1024,
        "MiB" => 1024 * 1024,
        "GiB" => 1024 * 1024 * 1024,
        "TiB" => 1024_u64.pow(4),
        _ => anyhow::bail!("unknown size unit {unit:?}, expected one of B, KiB, MiB, GiB, TiB"),
    };

    number
        .trim()
        .parse::<u64>()
        .map(|n| n * multiplier)
        .with_context(|| format!("invalid size value {number:?}"))
}

fn size_value(value: SizeValue) -> anyhow::Result<u64> {
    match value {
        SizeValue::Bytes(n) if n >= 0 => Ok(n as u64),
        SizeValue::Bytes(_) => anyhow::bail!("size cannot be negative"),
        SizeValue::Human(s) => parse_size(&s),
    }
}

fn deserialize_max_size<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    match SizeValue::deserialize(deserializer)? {
        SizeValue::Bytes(-1) => Ok(-1),
        SizeValue::Human(s) if s.trim() == "-1" => Ok(-1),
        value => {
            let size = size_value(value).map_err(serde::de::Error::custom)?;
            if size == 0 {
                return Err(serde::de::Error::custom(
                    "max_size must be -1 (unlimited) or a positive size",
                ));
            }
            Ok(size as i64)
        }
    }
}

fn deserialize_min_free_space<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    size_value(SizeValue::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

fn default_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        paths.push(PathBuf::from(config_home).join("greg-ng/config.toml"));
    } else if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".config/greg-ng/config.toml"));
    }

    paths.push(PathBuf::from("/etc/greg-ng/config.toml"));

    paths
}

fn validate(config: &Config) -> anyhow::Result<()> {
    if config.mpv.socket_path.is_none() && config.mpv.auto_start == Some(false) {
        anyhow::bail!(
            "Invalid `mpv` config: `auto_start` is set to false, but no `socket_path` was \
             given to connect to instead"
        );
    }

    if config.media_cache.enabled {
        if config.media_cache.stall_timeout_secs == 0 {
            anyhow::bail!(
                "Invalid `media_cache` config: `stall_timeout_secs` must be greater than zero"
            );
        }
        if config.media_cache.max_playlist_entries == 0 {
            anyhow::bail!(
                "Invalid `media_cache` config: `max_playlist_entries` must be greater than zero"
            );
        }
    }

    Ok(())
}

fn load_config_from(path: &Path) -> anyhow::Result<Config> {
    tracing::debug!("Loading config from {}", path.display());

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file at {}", path.display()))?;

    toml::from_str(&content)
        .with_context(|| format!("Failed to parse config file at {}", path.display()))
}

pub fn load_config(explicit_path: Option<&str>) -> anyhow::Result<Config> {
    let config = if let Some(path) = explicit_path {
        load_config_from(Path::new(path))?
    } else {
        match default_config_paths()
            .into_iter()
            .find(|path| path.exists())
        {
            Some(path) => load_config_from(&path)?,
            None => {
                tracing::debug!("No config file found, using default configuration");
                Config::default()
            }
        }
    };

    validate(&config)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_accepts_suffixes() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("1 KiB").unwrap(), 1024);
        assert_eq!(parse_size("512MiB").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_size("10 GiB").unwrap(), 10 * 1024_u64.pow(3));
        assert_eq!(parse_size("1 TiB").unwrap(), 1024_u64.pow(4));
    }

    #[test]
    fn parse_size_rejects_garbage() {
        assert!(parse_size("").is_err());
        assert!(parse_size("GiB").is_err());
        assert!(parse_size("10 XB").is_err());
        assert!(parse_size("ten GiB").is_err());
    }

    #[test]
    fn media_cache_config_defaults_from_empty_toml() {
        let config: Config = toml::from_str("").unwrap();
        assert!(!config.media_cache.enabled);
        assert_eq!(config.media_cache.max_size, -1);
        assert_eq!(config.media_cache.min_free_space, 1024 * 1024 * 1024);
        assert_eq!(config.media_cache.stall_timeout_secs, 30);
        assert_eq!(config.media_cache.max_playlist_entries, 500);
    }

    #[test]
    fn media_cache_config_parses_max_size() {
        let config: Config = toml::from_str("[media_cache]\nmax_size = \"2 GiB\"\n").unwrap();
        assert_eq!(config.media_cache.max_size, 2 * 1024_i64.pow(3));

        let config: Config = toml::from_str("[media_cache]\nmax_size = -1\n").unwrap();
        assert_eq!(config.media_cache.max_size, -1);
    }

    #[test]
    fn media_cache_rejects_invalid_max_size() {
        let toml = "[media_cache]\nenabled = true\nmax_size = 0\n";
        assert!(toml::from_str::<Config>(toml).is_err());
    }
}

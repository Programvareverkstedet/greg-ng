use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Deserializer};
use tempfile::NamedTempFile;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub mpv: MpvConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Hostname to bind the various APIs to.
    pub host: String,

    /// Port to bind the various APIs to.
    pub port: u16,

    #[serde(deserialize_with = "deserialize_level_filter")]
    pub verbosity: log::LevelFilter,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 8008,
            verbosity: log::LevelFilter::Error,
        }
    }
}

fn deserialize_level_filter<'de, D>(deserializer: D) -> Result<log::LevelFilter, D::Error>
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
    pub socket_path: String,

    /// Location of the mpv binary, used when auto-starting mpv.
    pub executable_path: Option<String>,

    /// An optional config file for mpv, used when auto-starting mpv.
    pub config_file: Option<String>,

    /// Instead of using the socket path, start a new private mpv instance
    /// with the given executable and config file.
    pub auto_start: bool,

    /// A generated temporary file that contains mpv config.
    /// Keeping it here makes the temporary file bound to the lifetime of
    /// the configuration, so we won't have to worry too much about it in
    /// the rest of the code. It's not expected to actually be part of the
    /// parsed config file.
    #[serde(skip)]
    pub resolved_config_file: Option<NamedTempFile>,
}

impl Default for MpvConfig {
    fn default() -> Self {
        Self {
            socket_path: "/run/mpv/mpv.sock".to_string(),
            executable_path: None,
            config_file: None,
            auto_start: true,
            resolved_config_file: None,
        }
    }
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

fn load_config_from(path: &Path) -> anyhow::Result<Config> {
    log::debug!("Loading config from {}", path.display());

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file at {}", path.display()))?;

    toml::from_str(&content)
        .with_context(|| format!("Failed to parse config file at {}", path.display()))
}

pub fn load_config(explicit_path: Option<&str>) -> anyhow::Result<Config> {
    if let Some(path) = explicit_path {
        return load_config_from(Path::new(path));
    }

    match default_config_paths()
        .into_iter()
        .find(|path| path.exists())
    {
        Some(path) => load_config_from(&path),
        None => {
            log::debug!("No config file found, using default configuration");
            Ok(Config::default())
        }
    }
}

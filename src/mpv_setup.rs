use std::{
    io::Write,
    os::fd::{AsRawFd, OwnedFd},
    path::Path,
};

use anyhow::Context;
use futures::StreamExt;
use mpvipc_async::{Event, EventLogMessageLevel, Mpv, MpvExt};
use nix::sys::socket::{AddressFamily, SockFlag, SockType, socketpair};
use tempfile::NamedTempFile;
use tokio::process::{Child, Command};

use crate::config::MpvConfig;

const DEFAULT_MPV_CONFIG_CONTENT: &str = include_str!("../assets/default-mpv.conf");

const THE_MAN_PNG: &[u8] = include_bytes!("../assets/the_man.png");

// https://mpv.io/manual/master/#options-ytdl
const YTDL_HOOK_ARGS: [&str; 2] = ["try_ytdl_first=yes", "thumbnails=none"];

impl MpvConfig {
    fn materialize_config_file(&mut self) -> anyhow::Result<()> {
        let file_content = match &self.config_file {
            Some(path) => {
                if !Path::new(path).exists() {
                    anyhow::bail!("Mpv config file not found at {}", path);
                }

                std::fs::read_to_string(path).context("Failed to read mpv config file")?
            }
            None => DEFAULT_MPV_CONFIG_CONTENT.to_string(),
        };

        let tmpfile = tempfile::Builder::new()
            .prefix("mpv-")
            .rand_bytes(8)
            .suffix(".conf")
            .tempfile()?;

        tmpfile.reopen()?.write_all(file_content.as_bytes())?;

        self.resolved_config_file = Some(tmpfile);
        Ok(())
    }
}

fn create_mpv_ipc_socketpair() -> anyhow::Result<(tokio::net::UnixStream, OwnedFd)> {
    let (tx_fd, rx_fd): (OwnedFd, OwnedFd) = socketpair(
        AddressFamily::Unix,
        SockType::Stream,
        None,
        SockFlag::empty(),
    )
    .context("Failed to create mpv IPC socketpair")?;

    let tx_std = std::os::unix::net::UnixStream::from(tx_fd);
    tx_std.set_nonblocking(true)?;
    let tx = tokio::net::UnixStream::from_std(tx_std)?;

    Ok((tx, rx_fd))
}

async fn spawn_mpv(
    executable_path: Option<&str>,
    config_file: &NamedTempFile,
    ytdlp_cookies_path: &Path,
) -> anyhow::Result<(Mpv, Child)> {
    let (tx, rx) = create_mpv_ipc_socketpair()?;

    tracing::info!(
        "Starting mpv with an internal IPC socket at fd://{}",
        rx.as_raw_fd()
    );

    let process_handle = Command::new(executable_path.unwrap_or("mpv"))
        .arg(format!("--input-ipc-client=fd://{}", rx.as_raw_fd()))
        .arg("--idle")
        .arg("--force-window")
        .arg("--fullscreen")
        .arg("--no-config")
        .arg("--no-terminal")
        .arg("--ytdl=yes")
        .args(
            YTDL_HOOK_ARGS
                .into_iter()
                .map(|x| format!("--script-opts=ytdl_hook-{}", x))
                .collect::<Vec<_>>(),
        )
        .arg(format!(
            "--include={}",
            config_file.path().to_string_lossy()
        ))
        .arg(format!(
            "--ytdl-raw-options=cookies={}",
            ytdlp_cookies_path.to_string_lossy()
        ))
        .arg("--load-unsafe-playlists")
        .arg("--keep-open") // Keep last frame of video on end of video
        .spawn()
        .context("Failed to start mpv")?;

    let mpv = Mpv::connect_socket(tx)
        .await
        .context("Failed to connect to mpv")?;

    Ok((mpv, process_handle))
}

async fn relay_mpv_log_messages(mpv: Mpv) -> anyhow::Result<()> {
    mpv.run_command_raw("request_log_messages", &["warn"])
        .await
        .context("Failed to subscribe to mpv log messages")?;

    tokio::spawn(async move {
        let mut events = mpv.get_event_stream().await;
        while let Some(event) = events.next().await {
            let Ok(Event::LogMessage {
                prefix,
                level,
                text,
            }) = event
            else {
                continue;
            };

            let text = text.trim_end();
            match level {
                EventLogMessageLevel::Fatal | EventLogMessageLevel::Error => {
                    tracing::error!(target: "mpv", "[{prefix}] {text}")
                }
                EventLogMessageLevel::Warn => tracing::warn!(target: "mpv", "[{prefix}] {text}"),
                EventLogMessageLevel::Info => tracing::info!(target: "mpv", "[{prefix}] {text}"),
                _ => tracing::debug!(target: "mpv", "[{prefix}] {text}"),
            }
        }
    });

    Ok(())
}

async fn connect_to_running_mpv(socket_path: &str) -> anyhow::Result<Mpv> {
    let path = Path::new(socket_path);

    if tokio::time::timeout(tokio::time::Duration::from_millis(500), async {
        while !path.exists() {
            tracing::debug!("Waiting for mpv socket at {}", socket_path);
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .is_err()
    {
        anyhow::bail!("Failed to connect to mpv socket: {}", socket_path);
    }

    Mpv::connect(socket_path)
        .await
        .context(format!("Failed to connect to mpv socket: {}", socket_path))
}

pub async fn connect_to_mpv(
    mpv_config: &mut MpvConfig,
    ytdlp_cookies_path: &Path,
) -> anyhow::Result<(Mpv, Option<Child>)> {
    tracing::debug!("Connecting to mpv");

    let (mpv, process_handle) = if mpv_config.should_auto_start() {
        mpv_config.materialize_config_file()?;
        let config_file = mpv_config
            .resolved_config_file
            .as_ref()
            .expect("config file was just materialized");

        let (mpv, process_handle) = spawn_mpv(
            mpv_config.executable_path.as_deref(),
            config_file,
            ytdlp_cookies_path,
        )
        .await?;
        (mpv, Some(process_handle))
    } else {
        let socket_path = mpv_config
            .socket_path
            .as_deref()
            .expect("validated at config load time");
        let mpv = connect_to_running_mpv(socket_path).await?;
        (mpv, None)
    };

    relay_mpv_log_messages(mpv.clone()).await?;

    if let Err(e) = mpv
        .set_property(
            "ytdl-raw-options",
            format!("cookies={}", ytdlp_cookies_path.to_string_lossy()),
        )
        .await
    {
        tracing::warn!("Failed to set yt-dlp cookies path on mpv: {e}");
    }

    Ok((mpv, process_handle))
}

pub async fn show_grzegorz_image(mpv: Mpv) -> anyhow::Result<()> {
    let path = std::env::temp_dir().join("the_man.png");
    std::fs::write(path.as_path(), THE_MAN_PNG)?;

    mpv.playlist_clear().await?;
    mpv.playlist_add(
        path.to_string_lossy().as_ref(),
        mpvipc_async::PlaylistAddTypeOptions::File,
        mpvipc_async::PlaylistAddOptions::Append,
    )
    .await?;
    mpv.next().await?;

    Ok(())
}

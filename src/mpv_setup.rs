use std::{fs::create_dir_all, io::Write, path::Path};

use anyhow::Context;
use mpvipc_async::{Mpv, MpvExt};
use tempfile::NamedTempFile;
use tokio::process::{Child, Command};

use crate::MpvConnectionArgs;

const DEFAULT_MPV_CONFIG_CONTENT: &str = include_str!("../assets/default-mpv.conf");

const THE_MAN_PNG: &[u8] = include_bytes!("../assets/the_man.png");

// https://mpv.io/manual/master/#options-ytdl
const YTDL_HOOK_ARGS: [&str; 2] = ["try_ytdl_first=yes", "thumbnails=none"];

pub fn create_mpv_config_file(args_config_file: Option<String>) -> anyhow::Result<NamedTempFile> {
    let file_content = if let Some(path) = args_config_file {
        if !Path::new(&path).exists() {
            anyhow::bail!("Mpv config file not found at {}", &path);
        }

        std::fs::read_to_string(&path).context("Failed to read mpv config file")?
    } else {
        DEFAULT_MPV_CONFIG_CONTENT.to_string()
    };

    let tmpfile = tempfile::Builder::new()
        .prefix("mpv-")
        .rand_bytes(8)
        .suffix(".conf")
        .tempfile()?;

    tmpfile.reopen()?.write_all(file_content.as_bytes())?;

    Ok(tmpfile)
}

pub async fn connect_to_mpv(args: &MpvConnectionArgs<'_>) -> anyhow::Result<(Mpv, Option<Child>)> {
    log::debug!("Connecting to mpv");

    debug_assert!(
        !args.force_auto_start || args.auto_start,
        "force_auto_start requires auto_start"
    );

    let socket_path = Path::new(&args.socket_path);

    if !socket_path.exists() {
        log::debug!("Mpv socket not found at {}", &args.socket_path);
        if !args.auto_start {
            panic!("Mpv socket not found at {}", &args.socket_path);
        }

        log::debug!("Ensuring parent dir of mpv socket exists");
        let parent_dir = Path::new(&args.socket_path)
            .parent()
            .context("Failed to get parent dir of mpv socket")?;

        if !parent_dir.is_dir() {
            create_dir_all(parent_dir).context("Failed to create parent dir of mpv socket")?;
        }
    } else {
        log::debug!("Existing mpv socket found at {}", &args.socket_path);
        if args.force_auto_start {
            log::debug!("Removing mpv socket");
            std::fs::remove_file(&args.socket_path)?;
        }
    }

    let process_handle = if args.auto_start {
        log::info!("Starting mpv with socket at {}", &args.socket_path);

        // TODO: try to fetch mpv from PATH
        Some(
            Command::new(args.executable_path.as_deref().unwrap_or("mpv"))
                .arg(format!("--input-ipc-server={}", &args.socket_path))
                .arg("--idle")
                .arg("--force-window")
                .arg("--fullscreen")
                .arg("--no-config")
                .arg("--ytdl=yes")
                .args(
                    YTDL_HOOK_ARGS
                        .into_iter()
                        .map(|x| format!("--script-opts=ytdl_hook-{}", x))
                        .collect::<Vec<_>>(),
                )
                .arg(format!(
                    "--include={}",
                    &args.config_file.path().to_string_lossy()
                ))
                // .arg("--no-terminal")
                .arg("--load-unsafe-playlists")
                .arg("--keep-open") // Keep last frame of video on end of video
                .spawn()
                .context("Failed to start mpv")?,
        )
    } else {
        None
    };

    // Wait for mpv to create the socket
    if tokio::time::timeout(tokio::time::Duration::from_millis(500), async {
        while !&socket_path.exists() {
            log::debug!("Waiting for mpv socket at {}", &args.socket_path);
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .is_err()
    {
        return Err(anyhow::anyhow!(
            "Failed to connect to mpv socket: {}",
            &args.socket_path
        ));
    }

    Ok((
        Mpv::connect(&args.socket_path).await.context(format!(
            "Failed to connect to mpv socket: {}",
            &args.socket_path
        ))?,
        process_handle,
    ))
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

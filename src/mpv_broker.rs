use std::{fs::create_dir_all, path::Path};

use anyhow::Context;
use mpvipc_async::{Mpv, MpvCommand, Event as MpvEvent};
use tokio::{process::{Child, Command}, sync::{broadcast::{Receiver as BroadcastReceiver, Sender as BroadcastSender}, mpsc::{Receiver as MpscReceiver, Sender as MpscSender}}};

#[derive(Debug)]
pub struct MpvBroker {
    mpv: Mpv,
    command_channel: MpscReceiver<MpvCommand>,
    event_listeners: BroadcastSender<MpvEvent>,
}

impl MpvBroker {
    pub fn new(
        mpv: Mpv,
        command_channel: MpscReceiver<MpvCommand>,
        event_listeners: BroadcastSender<MpvEvent>,
    ) -> Self {
        Self {
            mpv,
            command_channel,
            event_listeners,
        }
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                Some(command) = self.command_channel.recv() => {
                    self.mpv.run_command(command)?;
                }
                Ok(event) = async { self.mpv.event_listen() } => {
                    self.event_listeners.send(event)?;
                }
            }
        }
    }
}

pub struct MpvConnectionArgs {
    pub socket_path: String,
    pub executable_path: Option<String>,
    pub auto_start: bool,
    pub force_auto_start: bool,
}

pub async fn connect_to_mpv(args: &MpvConnectionArgs) -> anyhow::Result<(Mpv, Option<Child>)> {
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
                // .arg("--fullscreen")
                // .arg("--no-terminal")
                // .arg("--load-unsafe-playlists")
                .arg("--keep-open") // Keep last frame of video on end of video
                .arg("--really-quiet")
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
        Mpv::connect(&args.socket_path).context(format!(
            "Failed to connect to mpv socket: {}",
            &args.socket_path
        ))?,
        process_handle,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mpvipc_async::MpvCommand;
    use tokio::sync::{broadcast, mpsc};

    #[tokio::test]
    async fn test_run() -> anyhow::Result<()> {
        let (command_tx, command_rx) = mpsc::channel(1);
        let (event_tx, _) = broadcast::channel(1);

        let (mpv, _) = connect_to_mpv(&MpvConnectionArgs {
            socket_path: "/tmp/mpv-test.sock".to_string(),
            executable_path: None,
            auto_start: true,
            force_auto_start: true,
        }).await?;

        let mut broker = MpvBroker::new(mpv, command_rx, event_tx);
        let broker_handle = tokio::spawn(async move {
          broker.run().await.unwrap();
        });

        let _ = command_tx.send(MpvCommand::PlaylistClear).await;
        let _ = broker_handle.await.unwrap();

        Ok(())
    }
}
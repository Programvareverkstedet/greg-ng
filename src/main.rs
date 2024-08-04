use anyhow::Context;
use axum::{Router, Server};
use clap::Parser;
use futures::StreamExt;
use mpvipc_async::{parse_property, Mpv, MpvExt, Switch};
use std::{
    fs::create_dir_all,
    net::{IpAddr, SocketAddr},
    path::Path,
};
use tokio::process::{Child, Command};

mod api;

#[derive(Parser)]
struct Args {
    #[clap(long, default_value = "localhost")]
    host: String,

    #[clap(short, long, default_value = "8008")]
    port: u16,

    #[clap(long, value_name = "PATH", default_value = "/run/mpv/mpv.sock")]
    mpv_socket_path: String,

    #[clap(long, value_name = "PATH")]
    mpv_executable_path: Option<String>,

    #[clap(long, default_value = "true")]
    auto_start_mpv: bool,

    #[clap(long, default_value = "true")]
    force_auto_start: bool,
}

struct MpvConnectionArgs {
    socket_path: String,
    executable_path: Option<String>,
    auto_start: bool,
    force_auto_start: bool,
}

async fn connect_to_mpv(args: &MpvConnectionArgs) -> anyhow::Result<(Mpv, Option<Child>)> {
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

async fn resolve(host: &str) -> anyhow::Result<IpAddr> {
    let addr = format!("{}:0", host);
    let addresses = tokio::net::lookup_host(addr).await?;
    addresses
        .into_iter()
        .find(|addr| addr.is_ipv4())
        .map(|addr| addr.ip())
        .ok_or_else(|| anyhow::anyhow!("Failed to resolve address"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args = Args::parse();

    let (mpv, proc) = connect_to_mpv(&MpvConnectionArgs {
        socket_path: args.mpv_socket_path,
        executable_path: args.mpv_executable_path,
        auto_start: args.auto_start_mpv,
        force_auto_start: args.force_auto_start,
    })
    .await?;

    let addr = SocketAddr::new(resolve(&args.host).await?, args.port);
    log::info!("Starting API on {}", addr);

    let app = Router::new().nest("/api", api::rest_api_routes(mpv.clone()));

    if let Some(mut proc) = proc {
        tokio::select! {
            exit_status = proc.wait() => {
                log::warn!("mpv process exited with status: {}", exit_status?);
                mpv.disconnect().await?;
            }
            _ = tokio::signal::ctrl_c() => {
                log::info!("Received Ctrl-C, exiting");
                mpv.disconnect().await?;
                proc.kill().await?;
            }
          /* DEBUG */
          _ = async {
              let mut event_stream = mpv.get_event_stream().await;
              mpv.set_playback(Switch::Off).await.unwrap();
              mpv.observe_property(1, "volume").await.unwrap();
              mpv.observe_property(2, "pause").await.unwrap();
              mpv.observe_property(3, "time-pos").await.unwrap();
              mpv.observe_property(4, "duration").await.unwrap();
              mpv.observe_property(5, "playlist").await.unwrap();
              mpv.observe_property(6, "playlist-pos").await.unwrap();
              mpv.observe_property(7, "tick").await.unwrap();
              mpv.observe_property(8, "eof-reached").await.unwrap();
              mpv.observe_property(9, "speed").await.unwrap();
              mpv.observe_property(10, "filename").await.unwrap();
              mpv.observe_property(11, "media-title").await.unwrap();
              mpv.observe_property(12, "loop-file").await.unwrap();
              mpv.observe_property(13, "loop-playlist").await.unwrap();
              mpv.observe_property(14, "mute").await.unwrap();

              loop {
                let event = event_stream.next().await;
                if let Some(Ok(event)) = event {
                    match &event {
                      mpvipc_async::Event::PropertyChange { name, data, id } => {
                        let parsed_event_property = parse_property(name, data.clone());
                        log::info!("PropertyChange({}): {:#?}", id, parsed_event_property);
                      }
                      event => {
                        log::info!("Event: {:?}", event);
                      }
                    }
                }
              }
          } => {

          }
          /* END_DEBUG */
            result = async {
              match Server::try_bind(&addr.clone()).context("Failed to bind server") {
                Ok(server) => server.serve(app.into_make_service()).await.context("Failed to serve app"),
                Err(err) => Err(err),
              }
            } => {
              log::info!("API server exited");
              mpv.disconnect().await?;
              proc.kill().await?;
              result?;
            }
        }
    } else {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                log::info!("Received Ctrl-C, exiting");
                mpv.disconnect().await?;
            }
            _ =  Server::bind(&addr.clone()).serve(app.into_make_service()) => {
                log::info!("API server exited");
                mpv.disconnect().await?;
            }
        }
    }

    Ok(())
}

use anyhow::Context;
use axum::{Router, Server};
use clap::Parser;
use mpvipc::Mpv;
use std::{
    fs::create_dir_all,
    net::{IpAddr, SocketAddr},
    path::Path,
    sync::Arc,
};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

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
                // .arg("--fullscreen")
                // .arg("--no-terminal")
                // .arg("--load-unsafe-playlists")
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
        Mpv::connect(&args.socket_path).context(format!(
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

    let mpv = Arc::new(Mutex::new(mpv));
    let app = Router::new().nest("/api", api::rest_api_routes(mpv));

    if let Some(mut proc) = proc {
        tokio::select! {
          exit_status = proc.wait() => {
              log::warn!("mpv process exited with status: {}", exit_status?);
          }
          _ = tokio::signal::ctrl_c() => {
              log::info!("Received Ctrl-C, exiting");
              proc.kill().await?;
          }
          result = async {
              match Server::try_bind(&addr.clone()).context("Failed to bind server") {
                Ok(server) => server.serve(app.into_make_service()).await.context("Failed to serve app"),
                Err(err) => Err(err),
              }
            } => {
              log::info!("API server exited");
              proc.kill().await?;
              result?;
            }
        }
    } else {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                log::info!("Received Ctrl-C, exiting");
            }
            _ =  Server::bind(&addr.clone()).serve(app.into_make_service()) => {
                log::info!("API server exited");
            }
        }
    }

    Ok(())
}

use anyhow::Context;
use axum::{Router, Server};
use clap::Parser;
use mpv_setup::{connect_to_mpv, create_mpv_config_file, show_grzegorz_image};
use std::net::{IpAddr, SocketAddr};
use tempfile::NamedTempFile;

mod api;
mod mpv_setup;

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

    #[clap(long, value_name = "PATH")]
    mpv_config_file: Option<String>,

    #[clap(long, default_value = "true")]
    auto_start_mpv: bool,

    #[clap(long, default_value = "true")]
    force_auto_start: bool,
}

struct MpvConnectionArgs<'a> {
    socket_path: String,
    executable_path: Option<String>,
    config_file: &'a NamedTempFile,
    auto_start: bool,
    force_auto_start: bool,
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

    let mpv_config_file = create_mpv_config_file(args.mpv_config_file)?;

    let (mpv, proc) = connect_to_mpv(&MpvConnectionArgs {
        socket_path: args.mpv_socket_path,
        executable_path: args.mpv_executable_path,
        config_file: &mpv_config_file,
        auto_start: args.auto_start_mpv,
        force_auto_start: args.force_auto_start,
    })
    .await?;

    show_grzegorz_image(mpv.clone()).await?;

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

    std::mem::drop(mpv_config_file);

    Ok(())
}

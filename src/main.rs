use anyhow::Context;
use axum::Router;
use clap::Parser;
use clap_verbosity_flag::Verbosity;
use mpv_setup::{connect_to_mpv, create_mpv_config_file, show_grzegorz_image};
use mpvipc_async::Mpv;
use std::net::{IpAddr, SocketAddr};
use systemd_journal_logger::JournalLog;
use tempfile::NamedTempFile;

mod api;
mod mpv_setup;

#[derive(Parser)]
struct Args {
    #[clap(long, default_value = "localhost")]
    host: String,

    #[clap(short, long, default_value = "8008")]
    port: u16,

    #[command(flatten)]
    verbose: Verbosity,

    #[clap(long)]
    systemd: bool,

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

/// Helper function to resolve a hostname to an IP address.
/// Why is this not in the standard library? >:(
async fn resolve(host: &str) -> anyhow::Result<IpAddr> {
    let addr = format!("{}:0", host);
    let addresses = tokio::net::lookup_host(addr).await?;
    addresses
        .into_iter()
        .find(|addr| addr.is_ipv4())
        .map(|addr| addr.ip())
        .ok_or_else(|| anyhow::anyhow!("Failed to resolve address"))
}

/// Helper function that spawns a tokio thread that
/// continuously sends a ping to systemd watchdog, if enabled.
async fn setup_systemd_watchdog_thread() -> anyhow::Result<()> {
    let mut watchdog_microsecs: u64 = 0;
    if sd_notify::watchdog_enabled(true, &mut watchdog_microsecs) {
        watchdog_microsecs = watchdog_microsecs.div_ceil(2);
        tokio::spawn(async move {
            log::debug!(
                "Starting systemd watchdog thread with {} millisecond interval",
                watchdog_microsecs.div_ceil(1000)
            );
            loop {
                tokio::time::sleep(tokio::time::Duration::from_micros(watchdog_microsecs)).await;
                if let Err(err) = sd_notify::notify(false, &[sd_notify::NotifyState::Watchdog]) {
                    log::warn!("Failed to notify systemd watchdog: {}", err);
                } else {
                    log::trace!("Ping sent to systemd watchdog");
                }
            }
        });
    } else {
        log::info!("Watchdog not enabled, skipping");
    }
    Ok(())
}

async fn shutdown(mpv: Mpv, proc: Option<tokio::process::Child>) {
    log::info!("Shutting down");
    sd_notify::notify(false, &[sd_notify::NotifyState::Stopping]).unwrap_or_else(|e| {
        log::warn!(
            "Failed to notify systemd that the service is stopping: {}",
            e
        )
    });

    mpv.disconnect()
        .await
        .unwrap_or_else(|e| log::warn!("Failed to disconnect from mpv: {}", e));
    if let Some(mut proc) = proc {
        proc.kill()
            .await
            .unwrap_or_else(|e| log::warn!("Failed to kill mpv process: {}", e));
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let systemd_mode = args.systemd && sd_notify::booted().unwrap_or(false);
    if systemd_mode {
        JournalLog::new()
            .context("Failed to initialize journald logging")?
            .install()
            .context("Failed to install journald logger")?;

        log::set_max_level(args.verbose.log_level_filter());

        log::debug!("Running with systemd integration");

        setup_systemd_watchdog_thread().await?;
    } else {
        env_logger::Builder::new()
            .filter_level(args.verbose.log_level_filter())
            .init();

        log::info!("Running without systemd integration");
    }

    let mpv_config_file = create_mpv_config_file(args.mpv_config_file)?;

    let (mpv, proc) = connect_to_mpv(&MpvConnectionArgs {
        socket_path: args.mpv_socket_path,
        executable_path: args.mpv_executable_path,
        config_file: &mpv_config_file,
        auto_start: args.auto_start_mpv,
        force_auto_start: args.force_auto_start,
    })
    .await
    .context("Failed to connect to mpv")?;

    if let Err(e) = show_grzegorz_image(mpv.clone()).await {
        log::warn!("Could not show Grzegorz image: {}", e);
    }

    let addr = match resolve(&args.host)
        .await
        .context(format!("Failed to resolve address: {}", &args.host))
    {
        Ok(addr) => addr,
        Err(e) => {
            log::error!("{}", e);
            shutdown(mpv, proc).await;
            return Err(e);
        }
    };
    let socket_addr = SocketAddr::new(addr, args.port);
    log::info!("Starting API on {}", socket_addr);

    let app = Router::new()
        .nest("/api", api::rest_api_routes(mpv.clone()))
        .merge(api::rest_api_docs(mpv.clone()));

    let listener = match tokio::net::TcpListener::bind(&socket_addr)
        .await
        .context(format!("Failed to bind API server to '{}'", &socket_addr))
    {
        Ok(listener) => listener,
        Err(e) => {
            log::error!("{}", e);
            shutdown(mpv, proc).await;
            return Err(e);
        }
    };

    if systemd_mode {
        match sd_notify::notify(false, &[sd_notify::NotifyState::Ready])
            .context("Failed to notify systemd that the service is ready")
        {
            Ok(_) => log::trace!("Notified systemd that the service is ready"),
            Err(e) => {
                log::error!("{}", e);
                shutdown(mpv, proc).await;
                return Err(e);
            }
        }
    }

    if let Some(mut proc) = proc {
        tokio::select! {
            exit_status = proc.wait() => {
                log::warn!("mpv process exited with status: {}", exit_status?);
                shutdown(mpv, Some(proc)).await;
            }
            _ = tokio::signal::ctrl_c() => {
                log::info!("Received Ctrl-C, exiting");
                shutdown(mpv, Some(proc)).await;
            }
            result = axum::serve(listener, app.into_make_service()) => {
              log::info!("API server exited");
              shutdown(mpv, Some(proc)).await;
              result?;
            }
        }
    } else {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                log::info!("Received Ctrl-C, exiting");
                shutdown(mpv.clone(), None).await;
            }
            result = axum::serve(listener, app.into_make_service()) => {
              log::info!("API server exited");
              shutdown(mpv.clone(), None).await;
              result?;
            }
        }
    }

    std::mem::drop(mpv_config_file);

    Ok(())
}

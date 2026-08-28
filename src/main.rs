use anyhow::Context;
use axum::Router;
use clap::Parser;
use futures::StreamExt;
use mpv_setup::{connect_to_mpv, show_grzegorz_image};
use mpvipc_async::{Event, Mpv, MpvDataType, MpvExt};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
};
use tokio::{sync::mpsc, task::JoinHandle};
use tracing_subscriber::{Layer, filter::FilterExt, layer::SubscriberExt};
use util::{
    ConnectionEvent, HealthCheckRegistry, HealthCheckRequest, IdPool, YtDlpCookiesState,
    default_cookies_path, ensure_cookies_file_exists,
};

mod api;
mod config;
mod mpv_setup;
mod util;

const fn long_version() -> &'static str {
    const DIRTY_SUFFIX: &str = match option_env!("GIT_DIRTY") {
        Some(s) => match s.as_bytes() {
            b"true" => " (dirty)",
            _ => "",
        },
        None => "",
    };

    const BUILD_PROFILE: &str = match option_env!("BUILD_PROFILE") {
        Some(s) => s,
        None => "unknown",
    };

    const GIT_COMMIT: &str = match option_env!("GIT_COMMIT") {
        Some(s) => s,
        None => "unknown",
    };

    const GIT_COMMIT_DATE: &str = match option_env!("GIT_COMMIT_DATE") {
        Some(s) => s,
        None => "unknown",
    };

    const DEPENDENCY_LIST: &str = match option_env!("DEPENDENCY_LIST") {
        Some(s) => s,
        None => "",
    };

    const_format::concatcp!(
        env!("CARGO_PKG_VERSION"),
        "\n",
        "build profile: ",
        BUILD_PROFILE,
        "\n",
        "commit: ",
        GIT_COMMIT,
        DIRTY_SUFFIX,
        "\n",
        "commit date: ",
        GIT_COMMIT_DATE,
        "\n\n",
        "[dependencies]\n",
        const_format::str_replace!(DEPENDENCY_LIST, ";", "\n"),
    )
}

const LONG_VERSION: &str = long_version();

#[derive(Parser)]
#[command(version, long_version = LONG_VERSION)]
struct Args {
    #[clap(long, value_name = "PATH")]
    config: Option<String>,

    /// Enable systemd integration (watchdog, status updates, native logging, etc.)
    #[clap(long, action)]
    systemd: bool,

    /// Spawn mpv without a visible window.
    ///
    /// Mostly useful for testing, this won't affect external mpv instances connected by socket.
    #[clap(long, action)]
    headless: bool,
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

fn setup_mpv_health_check_thread(mpv: Mpv, mut requests: mpsc::Receiver<HealthCheckRequest>) {
    tokio::spawn(async move {
        while let Some(reply_tx) = requests.recv().await {
            let result = mpv
                .run_command_raw("get_time_us", &[])
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = reply_tx.send(result);
        }
    });
}

async fn setup_systemd_watchdog_thread(health_checks: HealthCheckRegistry) -> anyhow::Result<()> {
    if let Some(watchdog_interval) = sd_notify::watchdog_enabled() {
        let ping_interval = watchdog_interval / 2;
        let check_timeout = ping_interval / 2;
        tokio::spawn(async move {
            tracing::debug!(
                "Starting systemd watchdog thread with {} millisecond interval",
                ping_interval.as_millis()
            );
            loop {
                tokio::time::sleep(ping_interval).await;

                match health_checks.all_healthy(check_timeout).await {
                    Ok(()) => {
                        if let Err(err) = sd_notify::notify(&[sd_notify::NotifyState::Watchdog]) {
                            tracing::warn!("Failed to notify systemd watchdog: {}", err);
                        } else {
                            tracing::trace!("Ping sent to systemd watchdog");
                        }
                    }
                    Err(failed_check) => {
                        tracing::warn!(
                            "Skipping systemd watchdog ping, health check {:?} did not pass",
                            failed_check
                        );
                    }
                }
            }
        });
    } else {
        tracing::info!("Watchdog not enabled, skipping");
    }
    Ok(())
}

fn send_play_status(
    systemd: bool,
    playing: bool,
    current_song: &Option<String>,
    connection_count: u64,
    play_count: u64,
) {
    let status = &format!(
        "[CONN: {}] [PLAYS: {}] {} {:?}",
        connection_count,
        play_count,
        if playing { "[▶]" } else { "[⏸]" },
        if let Some(song) = current_song {
            song
        } else {
            ""
        }
    );

    if systemd {
        sd_notify::notify(&[sd_notify::NotifyState::Status(status)]).unwrap_or_else(|e| {
            tracing::warn!("Failed to update systemd status with current song: {}", e)
        });
    } else {
        tracing::info!("{}", status);
    }
}

async fn start_status_notifier_thread(
    systemd: bool,
    mpv: Mpv,
    mut connection_counter_rx: mpsc::Receiver<ConnectionEvent>,
) -> anyhow::Result<JoinHandle<()>> {
    let handle = tokio::spawn(async move {
        tracing::debug!("Starting systemd notifier thread");
        let mut event_stream = mpv.get_event_stream().await;

        mpv.observe_property(100, "media-title").await.unwrap();
        mpv.observe_property(100, "pause").await.unwrap();

        let mut current_song: Option<String> = mpv.get_property("media-title").await.unwrap();
        let mut playing = !mpv.get_property("pause").await.unwrap().unwrap_or(false);
        let mut connection_count = 0;
        let mut play_count = 0;

        send_play_status(
            systemd,
            playing,
            &current_song,
            connection_count,
            play_count,
        );

        loop {
            tokio::select! {
                event = event_stream.next() => {
                    match event {
                        Some(Ok(Event::PropertyChange { name, data, .. })) => {
                            match (name.as_str(), data) {
                                ("media-title", Some(MpvDataType::String(s))) => {
                                    if current_song.as_deref() != Some(s.as_str()) {
                                        play_count += 1;
                                        tracing::info!("Now playing: {}", s);
                                    }
                                    current_song = Some(s);
                                }
                                ("media-title", None) => {
                                    if current_song.is_some() {
                                        tracing::info!("Stopped playback");
                                    }
                                    current_song = None;
                                }
                                ("pause", Some(MpvDataType::Bool(b))) => {
                                    let now_playing = !b;
                                    if playing != now_playing {
                                        tracing::info!("{}", if now_playing { "Resumed playback" } else { "Paused playback" });
                                    }
                                    playing = now_playing;
                                }
                                (event_name, _) => {
                                    tracing::trace!(
                                        "Received unexpected property change on systemd notifier thread: {}",
                                        event_name
                                    );
                                }
                            }

                            send_play_status(systemd, playing, &current_song, connection_count, play_count);
                        }
                        Some(Ok(other)) => {
                            tracing::trace!(
                                "Received unexpected event on systemd notifier thread: {:?}",
                                other
                            );
                        }
                        Some(Err(e)) => {
                            tracing::warn!(
                                "Error reading event stream on systemd notifier thread: {}",
                                e
                            );
                        }
                        None => {
                            tracing::debug!("Event stream ended on systemd notifier thread");
                        }
                    }
                }

                connection_counter_update = connection_counter_rx.recv() => {
                    let Some(connection_counter_update) = connection_counter_update else {
                        tracing::debug!("Connection counter channel closed on systemd notifier thread");
                        continue;
                    };

                    tracing::trace!("Received connection counter update: {}", connection_counter_update);

                    match connection_count.checked_add_signed(connection_counter_update.to_i8().into()) {
                        Some(new_count) => connection_count = new_count,
                        None => {
                            tracing::warn!("Invalid connection count: trying to add {} to {}", connection_counter_update.to_i8(), connection_count);
                            tracing::warn!("Resetting connection count to 0");
                            connection_count = 0;
                        }
                    }

                    match connection_count {
                        0 => tracing::debug!("No connections"),
                        _ => tracing::debug!("Connection count: {}", connection_count),
                    }

                    send_play_status(systemd, playing, &current_song, connection_count, play_count);
                }
            }
        }
    });

    Ok(handle)
}

async fn shutdown(mpv: Mpv, proc: Option<tokio::process::Child>) {
    tracing::info!("Shutting down");
    sd_notify::notify(&[sd_notify::NotifyState::Stopping]).unwrap_or_else(|e| {
        tracing::warn!(
            "Failed to notify systemd that the service is stopping: {}",
            e
        )
    });

    mpv.disconnect()
        .await
        .unwrap_or_else(|e| tracing::warn!("Failed to disconnect from mpv: {}", e));
    if let Some(mut proc) = proc {
        proc.kill()
            .await
            .unwrap_or_else(|e| tracing::warn!("Failed to kill mpv process: {}", e));
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut config = config::load_config(args.config.as_deref())?;

    let log_level = config.server.verbosity;

    if args.systemd {
        let subscriber = tracing_subscriber::Registry::default()
            .with(log_level)
            .with(tracing_journald::layer().context("Failed to connect to journald")?);
        tracing::subscriber::set_global_default(subscriber)
            .context("Failed to set global default tracing subscriber")?;

        tracing::debug!("Running with systemd integration");
    } else {
        let filter = tracing_subscriber::EnvFilter::builder()
            .with_default_directive(log_level.into())
            .from_env_lossy();

        let is_mpv = tracing_subscriber::filter::filter_fn(|meta| meta.target() == "mpv");
        let mpv_layer = tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_file(true)
            .with_line_number(true)
            .with_filter(is_mpv.clone());
        let other_layer = tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_file(true)
            .with_line_number(true)
            .with_filter(is_mpv.not());

        let subscriber = tracing_subscriber::Registry::default()
            .with(filter)
            .with(mpv_layer)
            .with(other_layer);
        tracing::subscriber::set_global_default(subscriber)
            .context("Failed to set global default tracing subscriber")?;

        tracing::info!("Running without systemd integration");
    }

    let cookies_path = default_cookies_path();
    ensure_cookies_file_exists(&cookies_path).context("Failed to create yt-dlp cookies file")?;
    let (mpv, proc) = connect_to_mpv(&mut config.mpv, &cookies_path, args.headless)
        .await
        .context("Failed to connect to mpv")?;

    let health_checks = HealthCheckRegistry::new();
    let mpv_health_check_rx = health_checks.register("mpv-ipc");
    setup_mpv_health_check_thread(mpv.clone(), mpv_health_check_rx);

    if args.systemd {
        setup_systemd_watchdog_thread(health_checks.clone()).await?;
    }

    let (connection_counter_tx, connection_counter_rx) = mpsc::channel(10);

    let status_notifier_thread_handle =
        start_status_notifier_thread(args.systemd, mpv.clone(), connection_counter_rx).await?;

    if let Err(e) = show_grzegorz_image(mpv.clone()).await {
        tracing::warn!("Could not show Grzegorz image: {}", e);
    }

    let addr = match resolve(&config.server.host)
        .await
        .context(format!("Failed to resolve address: {}", config.server.host))
    {
        Ok(addr) => addr,
        Err(e) => {
            tracing::error!("{}", e);
            shutdown(mpv, proc).await;
            return Err(e);
        }
    };
    let socket_addr = SocketAddr::new(addr, config.server.port);
    tracing::info!("Starting API on {}", socket_addr);

    let id_pool = Arc::new(Mutex::new(IdPool::new_with_max_limit(1024)));
    let yt_dlp_cookies = YtDlpCookiesState::load(cookies_path);

    let app = Router::new()
        .nest(
            "/api",
            api::rest_api_routes(mpv.clone(), yt_dlp_cookies.clone()),
        )
        .nest(
            "/ws",
            api::websocket_api(
                mpv.clone(),
                id_pool.clone(),
                connection_counter_tx.clone(),
                yt_dlp_cookies.clone(),
            ),
        )
        .merge(api::health_routes(health_checks))
        .merge(api::rest_api_docs(mpv.clone(), yt_dlp_cookies))
        .into_make_service_with_connect_info::<SocketAddr>();

    let listener = match tokio::net::TcpListener::bind(&socket_addr)
        .await
        .context(format!("Failed to bind API server to '{}'", socket_addr))
    {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!("{}", e);
            shutdown(mpv, proc).await;
            return Err(e);
        }
    };

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("Failed to install SIGTERM handler")?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .context("Failed to install SIGINT handler")?;

    if args.systemd {
        match sd_notify::notify(&[sd_notify::NotifyState::Ready])
            .context("Failed to notify systemd that the service is ready")
        {
            Ok(_) => tracing::trace!("Notified systemd that the service is ready"),
            Err(e) => {
                tracing::error!("{}", e);
                shutdown(mpv, proc).await;
                return Err(e);
            }
        }
    }

    if let Some(mut proc) = proc {
        tokio::select! {
            exit_status = proc.wait() => {
                tracing::warn!("mpv process exited with status: {}", exit_status?);
                shutdown(mpv, Some(proc)).await;
            }
            _ = sigint.recv() => {
                tracing::info!("Received SIGINT, exiting");
                shutdown(mpv, Some(proc)).await;
            }
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM, exiting");
                shutdown(mpv, Some(proc)).await;
            }
            result = axum::serve(listener, app) => {
              tracing::info!("API server exited");
              shutdown(mpv, Some(proc)).await;
              result?;
            }
            result = status_notifier_thread_handle => {
              tracing::info!("Status notifier thread exited unexpectedly, shutting dow");
              shutdown(mpv, Some(proc)).await;
              result?;
            }
        }
    } else {
        tokio::select! {
            _ = sigint.recv() => {
                tracing::info!("Received SIGINT, exiting");
                shutdown(mpv.clone(), None).await;
            }
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM, exiting");
                shutdown(mpv.clone(), None).await;
            }
            result = axum::serve(listener, app) => {
              tracing::info!("API server exited");
              shutdown(mpv.clone(), None).await;
              result?;
            }
            result = status_notifier_thread_handle => {
              tracing::info!("Status notifier thread exited unexpectedly, shutting down");
              shutdown(mpv.clone(), None).await;
              result?;
            }
        }
    }

    Ok(())
}

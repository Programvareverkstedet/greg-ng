use std::{
    net::{IpAddr, SocketAddr},
    os::fd::FromRawFd,
    sync::Arc,
};

use anyhow::Context;
use axum::{extract::connect_info::Connected, serve::IncomingStream};
use nix::sys::socket::SockaddrLike;
use tokio::net::{TcpListener, UnixListener};

use crate::config::ServerConfig;

pub enum ApiListener {
    Tcp(TcpListener),
    Unix(UnixListener),
}

#[derive(Debug, Clone)]
pub enum ApiClientAddr {
    Tcp(SocketAddr),
    Unix(Arc<str>),
}

impl std::fmt::Display for ApiClientAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiClientAddr::Tcp(addr) => write!(f, "{addr}"),
            ApiClientAddr::Unix(label) => write!(f, "unix:{label}"),
        }
    }
}

fn unix_addr_label(addr: &tokio::net::unix::SocketAddr) -> Arc<str> {
    addr.as_pathname()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "(unnamed)".to_string())
        .into()
}

impl Connected<IncomingStream<'_, TcpListener>> for ApiClientAddr {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        ApiClientAddr::Tcp(*stream.remote_addr())
    }
}

impl Connected<IncomingStream<'_, UnixListener>> for ApiClientAddr {
    fn connect_info(stream: IncomingStream<'_, UnixListener>) -> Self {
        ApiClientAddr::Unix(unix_addr_label(stream.remote_addr()))
    }
}

impl ApiListener {
    fn local_addr(&self) -> std::io::Result<ApiClientAddr> {
        Ok(match self {
            ApiListener::Tcp(listener) => ApiClientAddr::Tcp(listener.local_addr()?),
            ApiListener::Unix(listener) => {
                ApiClientAddr::Unix(unix_addr_label(&listener.local_addr()?))
            }
        })
    }
}

fn listener_from_raw_fd(fd: std::os::fd::RawFd) -> anyhow::Result<ApiListener> {
    let sockaddr: nix::sys::socket::SockaddrStorage = nix::sys::socket::getsockname(fd)
        .context("Failed to inspect socket-activated file descriptor")?;

    match sockaddr.family() {
        Some(nix::sys::socket::AddressFamily::Inet)
        | Some(nix::sys::socket::AddressFamily::Inet6) => {
            let std_listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
            std_listener.set_nonblocking(true)?;
            let listener = tokio::net::TcpListener::from_std(std_listener)
                .context("Failed to adopt socket-activated TCP listener")?;
            Ok(ApiListener::Tcp(listener))
        }
        Some(nix::sys::socket::AddressFamily::Unix) => {
            let std_listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) };
            std_listener.set_nonblocking(true)?;
            let listener = tokio::net::UnixListener::from_std(std_listener)
                .context("Failed to adopt socket-activated Unix listener")?;
            Ok(ApiListener::Unix(listener))
        }
        other => anyhow::bail!(
            "Unsupported socket-activated file descriptor family: {:?}",
            other
        ),
    }
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

/// Either receives a socket-activated listener from systemd, or binds a
/// fresh TCP listener to the configured host and port.
pub async fn bind_listener(server: &ServerConfig) -> anyhow::Result<ApiListener> {
    let mut fds =
        sd_notify::listen_fds().context("Failed to check for systemd socket activation")?;

    if let Some(fd) = fds.next() {
        if fds.next().is_some() {
            anyhow::bail!(
                "Received more than one socket-activated file descriptor from systemd, expected only one"
            );
        }

        let listener = listener_from_raw_fd(fd)?;
        tracing::info!(
            "Using socket-activated listener from systemd on {}",
            listener.local_addr()?
        );
        return Ok(listener);
    }

    let addr = resolve(&server.host)
        .await
        .context(format!("Failed to resolve address: {}", server.host))?;
    let socket_addr = SocketAddr::new(addr, server.port);
    tracing::info!("Starting API on {}", socket_addr);

    let listener = tokio::net::TcpListener::bind(&socket_addr)
        .await
        .context(format!("Failed to bind API server to '{}'", socket_addr))?;
    Ok(ApiListener::Tcp(listener))
}

//! UDP DNS server: receives packets, dispatches to resolver, sends responses.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use arc_swap::ArcSwap;
use tokio::net::UdpSocket;
use tracing::{error, info, warn};

use crate::cache::DnsCache;
use crate::config::{self, Config};
use crate::dns::Resolver;

/// State shared across tasks
pub struct AppState {
    pub config: ArcSwap<Config>,
    pub cache: Arc<DnsCache>,
    pub resolver: Resolver,
    pub start_time: std::time::Instant,
    pub query_count: std::sync::atomic::AtomicU64,
}

pub async fn run(
    cfg: Config,
    host: String,
    port: u16,
    no_cache: bool,
    config_path: PathBuf,
) -> Result<()> {
    let cache_enabled = cfg.server.cache_enabled && !no_cache;
    let cache = Arc::new(DnsCache::new(
        cfg.server.cache_size,
        cfg.server.cache_ttl,
        cache_enabled,
    ));
    let resolver = Resolver::new(cache.clone());
    let mgmt_port = cfg.server.mgmt_port;
    let mgmt_host = cfg
        .server
        .mgmt_host
        .clone()
        .unwrap_or_else(|| "0.0.0.0".into());
    let hot_reload = cfg.server.hot_reload;
    let peers = cfg.server.peers.clone();

    let state = Arc::new(AppState {
        config: ArcSwap::new(Arc::new(cfg)),
        cache: cache.clone(),
        resolver,
        start_time: std::time::Instant::now(),
        query_count: std::sync::atomic::AtomicU64::new(0),
    });

    // ── DNS UDP listener ──────────────────────────────────────────────────────
    let bind_addr = format!("{}:{}", host, port);
    let socket = UdpSocket::bind(&bind_addr).await?;
    info!("DNS server listening on udp://{}", bind_addr);
    let socket = Arc::new(socket);

    // ── Hot-reload watcher ────────────────────────────────────────────────────
    if hot_reload {
        let state2 = state.clone();
        let path = config_path.clone();
        tokio::spawn(async move {
            watch_config(path, state2).await;
        });
    }

    // ── Management API ────────────────────────────────────────────────────────
    if let Some(mp) = mgmt_port {
        let state3 = state.clone();
        let config_path2 = config_path.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::mgmt::start(state3, &mgmt_host, mp, config_path2).await {
                error!("Management API error: {}", e);
            }
        });
    }

    // ── Peer sync ─────────────────────────────────────────────────────────────
    if !peers.is_empty() {
        let state4 = state.clone();
        tokio::spawn(async move {
            crate::sync::reconcile_loop(state4, peers).await;
        });
    }

    // ── Main receive loop ─────────────────────────────────────────────────────
    let mut buf = vec![0u8; 4096];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((n, src)) => {
                let query = buf[..n].to_vec();
                let sock = socket.clone();
                let state = state.clone();
                tokio::spawn(async move {
                    handle_packet(query, src, sock, state).await;
                });
            }
            Err(e) => {
                // On Windows, sending a UDP packet to a closed port causes the OS to
                // deliver a WSAECONNRESET (10054) error on the *next* recv_from call.
                // This is a benign Windows-specific behaviour — log at debug and continue.
                #[cfg(windows)]
                if let Some(raw) = e.raw_os_error() {
                    if raw == 10054 {
                        #[cfg(windows)]
                        tracing::debug!(
                            "UDP WSAECONNRESET (ICMP port-unreachable received) — ignored"
                        );
                        continue;
                    }
                }
                // Any other IO error: log as warning and keep running
                warn!("UDP recv error: {}", e);
            }
        }
    }
}

async fn handle_packet(
    query: Vec<u8>,
    src: SocketAddr,
    socket: Arc<UdpSocket>,
    state: Arc<AppState>,
) {
    let config = state.config.load();
    state
        .query_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let response = state.resolver.resolve(&query, &config).await;

    if response.is_empty() {
        return;
    }

    if let Err(e) = socket.send_to(&response, src).await {
        warn!("Failed to send response to {}: {}", src, e);
    }
}

/// Poll the config file every 5 s and hot-reload on mtime change.
async fn watch_config(path: PathBuf, state: Arc<AppState>) {
    let mut last_modified = get_mtime(&path);
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let current = get_mtime(&path);
        if current != last_modified {
            last_modified = current;
            info!("Config file changed, reloading...");
            match config::load(&path) {
                Ok(new_cfg) => {
                    state.cache.invalidate();
                    state.config.store(Arc::new(new_cfg));
                    info!("Config reloaded successfully");
                }
                Err(e) => {
                    warn!("Config reload failed: {} — keeping previous config", e);
                }
            }
        }
    }
}

fn get_mtime(path: &PathBuf) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

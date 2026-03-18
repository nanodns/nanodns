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

/// State shared across all async tasks
pub struct AppState {
    pub config: ArcSwap<Config>,
    pub cache: Arc<DnsCache>,
    pub resolver: Resolver,
    pub start_time: std::time::Instant,
    pub query_count: std::sync::atomic::AtomicU64,
}

/// Run the DNS server. All runtime parameters come from `cfg` (already merged
/// with CLI overrides by main.rs before this is called).
pub async fn run(cfg: Config, no_cache: bool, config_path: PathBuf) -> Result<()> {
    let cache_enabled = cfg.server.cache_enabled && !no_cache;
    let cache = Arc::new(DnsCache::new(
        cfg.server.cache_size,
        cfg.server.cache_ttl,
        cache_enabled,
    ));
    let resolver = Resolver::new(cache.clone());

    // Snapshot fields needed by sub-tasks before cfg is moved into ArcSwap
    let mgmt_enabled = cfg.server.mgmt_port > 0;
    let mgmt_addr = format!("{}:{}", cfg.server.mgmt_host, cfg.server.mgmt_port);
    let hot_reload = cfg.server.hot_reload;
    let peers = cfg.server.peers.clone();
    let bind_addr = format!("{}:{}", cfg.server.host, cfg.server.port);

    let state = Arc::new(AppState {
        config: ArcSwap::new(Arc::new(cfg)),
        cache: cache.clone(),
        resolver,
        start_time: std::time::Instant::now(),
        query_count: std::sync::atomic::AtomicU64::new(0),
    });

    // ── DNS UDP listener ──────────────────────────────────────────────────────
    let socket = UdpSocket::bind(&bind_addr).await?;
    info!("DNS listening on udp://{}", bind_addr);
    let socket = Arc::new(socket);

    // ── Hot-reload watcher ────────────────────────────────────────────────────
    if hot_reload {
        let state2 = state.clone();
        let path = config_path.clone();
        tokio::spawn(async move { watch_config(path, state2).await });
    }

    // ── Management HTTP API ───────────────────────────────────────────────────
    if mgmt_enabled {
        let state3 = state.clone();
        let config_path2 = config_path.clone();
        let mgmt_addr2 = mgmt_addr.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::mgmt::start(state3, &mgmt_addr2, config_path2).await {
                error!("Management API error: {}", e);
            }
        });
    }

    // ── Peer sync reconcile loop ──────────────────────────────────────────────
    if !peers.is_empty() {
        let state4 = state.clone();
        tokio::spawn(async move { crate::sync::reconcile_loop(state4, peers).await });
    }

    // ── Main UDP receive loop ─────────────────────────────────────────────────
    let mut buf = vec![0u8; 4096];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((n, src)) => {
                let query = buf[..n].to_vec();
                let sock = socket.clone();
                let state = state.clone();
                tokio::spawn(async move { handle_packet(query, src, sock, state).await });
            }
            Err(e) => {
                // Windows: WSAECONNRESET (10054) on UDP is benign — previous send
                // hit a closed port, the ICMP reply surfaces here. Ignore it.
                #[cfg(windows)]
                if let Some(raw) = e.raw_os_error() {
                    if raw == 10054 {
                        tracing::debug!("UDP WSAECONNRESET — ignored");
                        continue;
                    }
                }
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
    state
        .query_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let config = state.config.load();
    let response = state.resolver.resolve(&query, &config).await;
    if response.is_empty() {
        return;
    }
    if let Err(e) = socket.send_to(&response, src).await {
        warn!("Send to {} failed: {}", src, e);
    }
}

/// Poll config file every 5 s; hot-reload on mtime change.
async fn watch_config(path: PathBuf, state: Arc<AppState>) {
    let mut last_mtime = mtime(&path);
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let cur = mtime(&path);
        if cur != last_mtime {
            last_mtime = cur;
            info!("Config changed — reloading...");
            match config::load(&path) {
                Ok(mut new_cfg) => {
                    // Bump config_version — same as /reload endpoint does
                    let new_version = state.config.load().server.config_version + 1;
                    new_cfg.server.config_version = new_version;

                    let peers = new_cfg.server.peers.clone();
                    state.cache.invalidate();
                    state.config.store(Arc::new(new_cfg));
                    info!("Config reloaded — config_version now {}", new_version);

                    // Push to peers immediately (same behaviour as /reload)
                    if !peers.is_empty() {
                        let cfg_snapshot = (*state.config.load()).clone();
                        tokio::spawn(async move {
                            crate::sync::push_to_peers(&cfg_snapshot, &peers).await;
                        });
                    }
                }
                Err(e) => warn!("Config reload failed: {} — keeping current config", e),
            }
        }
    }
}

fn mtime(path: &PathBuf) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

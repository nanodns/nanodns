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
    /// Path to the config file — needed to persist config_version after sync
    pub config_path: PathBuf,
    /// Last known mtime of the config file — updated after any programmatic
    /// write (persist_version) so watch_config doesn't re-trigger on our own writes
    pub last_mtime: std::sync::Mutex<Option<std::time::SystemTime>>,
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
        config_path: config_path.clone(),
        last_mtime: std::sync::Mutex::new(mtime(&config_path)),
    });

    // ── DNS UDP listener ──────────────────────────────────────────────────────
    let socket = UdpSocket::bind(&bind_addr).await?;
    info!("DNS listening on udp://{}", bind_addr);
    let socket = Arc::new(socket);

    // ── Hot-reload watcher ────────────────────────────────────────────────────
    if hot_reload {
        let state2 = state.clone();
        tokio::spawn(async move { watch_config(state2).await });
    }

    // ── Management HTTP API ───────────────────────────────────────────────────
    if mgmt_enabled {
        let state3 = state.clone();
        let mgmt_addr2 = mgmt_addr.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::mgmt::start(state3, &mgmt_addr2).await {
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

/// Poll config file every 5 s; hot-reload only on user-driven mtime changes.
/// Uses `state.last_mtime` so that programmatic writes (persist_version, peer sync)
/// can update the baseline and prevent false reload triggers.
async fn watch_config(state: Arc<AppState>) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let cur = mtime(&state.config_path);
        let changed = {
            let known = state.last_mtime.lock().unwrap();
            cur != *known
        };
        if !changed {
            continue;
        }

        info!("Config changed — reloading...");
        match config::load(&state.config_path) {
            Ok(mut new_cfg) => {
                let new_version = state.config.load().server.config_version + 1;
                new_cfg.server.config_version = new_version;

                let peers = new_cfg.server.peers.clone();
                state.cache.invalidate();
                state.config.store(Arc::new(new_cfg));
                info!("Config reloaded — config_version now {}", new_version);

                // Persist version — this changes mtime again, so update our baseline
                if let Err(e) = config::persist_version(&state.config_path, new_version) {
                    warn!("Could not persist config_version: {}", e);
                }
                // Update last_mtime to the post-write mtime so we don't re-trigger
                *state.last_mtime.lock().unwrap() = mtime(&state.config_path);

                if !peers.is_empty() {
                    let cfg_snapshot = (*state.config.load()).clone();
                    tokio::spawn(async move {
                        crate::sync::push_to_peers(&cfg_snapshot, &peers).await;
                    });
                }
            }
            Err(e) => {
                // Even on parse failure, update mtime so we don't spam retries
                *state.last_mtime.lock().unwrap() = cur;
                warn!("Config reload failed: {} — keeping current config", e);
            }
        }
    }
}

pub fn mtime(path: &PathBuf) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

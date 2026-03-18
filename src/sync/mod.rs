//! Peer-to-peer config synchronisation — matches original Python NanoDNS HA behaviour.
//!
//! ## How it works (from USAGE.md)
//!
//! 1. **Push on reload**: when /reload is called, the config version is bumped and
//!    the new config is pushed to every online peer via POST /sync immediately (<1 s).
//!
//! 2. **Reconcile loop** (every 30 s): each node probes all peers' versions.
//!    If any peer has a *higher* version, we pull their full config and apply it.
//!    This catches up nodes that were offline during a reload.
//!
//! 3. **Version wins**: highest `config_version` always takes precedence.
//!    No split-brain is possible because only the node receiving `/reload` bumps
//!    the version; peers only ever accept strictly higher versions.
//!
//! 4. **Idempotent push**: if the checksum of the in-memory config equals the
//!    on-disk file, hot-reload does not bump the version or push to peers.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::server::AppState;

const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);
const PEER_TIMEOUT: Duration = Duration::from_secs(5);

// ─── Wire types ───────────────────────────────────────────────────────────────

/// Payload sent to a peer's POST /sync endpoint.
#[derive(Serialize, Deserialize)]
pub struct SyncPayload {
    pub config_version: u64,
    pub config: Config,
}

/// Minimal response from a peer's GET /metrics endpoint.
#[derive(Deserialize)]
struct PeerMetrics {
    config_version: u64,
}

// ─── Push ─────────────────────────────────────────────────────────────────────

/// Push `cfg` to every peer listed in `peers`.
/// Called immediately after a successful `/reload`.
pub async fn push_to_peers(cfg: &Config, peers: &[String]) {
    let client = build_client();
    let payload = SyncPayload {
        config_version: cfg.server.config_version,
        config: cfg.clone(),
    };

    for peer in peers {
        let url = format!("http://{}/sync", peer);
        match client.post(&url).json(&payload).send().await {
            Ok(r) if r.status().is_success() => {
                info!(
                    "Pushed config v{} to peer {}",
                    cfg.server.config_version, peer
                );
            }
            Ok(r) => warn!("Peer {} rejected sync: HTTP {}", peer, r.status()),
            Err(e) => debug!("Peer {} unreachable: {}", peer, e),
        }
    }
}

// ─── Version probe ────────────────────────────────────────────────────────────

/// Fetch just `config_version` from a peer's /metrics.
pub async fn fetch_peer_version(peer: &str) -> anyhow::Result<u64> {
    let url = format!("http://{}/metrics", peer);
    let resp: PeerMetrics = build_client().get(&url).send().await?.json().await?;
    Ok(resp.config_version)
}

/// Fetch the full config from a peer's /config/raw.
pub async fn fetch_peer_config(peer: &str) -> anyhow::Result<Config> {
    let url = format!("http://{}/config/raw", peer);
    let cfg: Config = build_client().get(&url).send().await?.json().await?;
    Ok(cfg)
}

// ─── Reconcile loop ───────────────────────────────────────────────────────────

/// Background task: runs every 30 s.
/// If any peer has a higher config_version, pull and apply their config.
/// This is the catch-up path for nodes that were offline during a `/reload`.
pub async fn reconcile_loop(state: Arc<AppState>, peers: Vec<String>) {
    info!("Peer reconcile loop started ({} peer(s))", peers.len());
    loop {
        tokio::time::sleep(RECONCILE_INTERVAL).await;
        reconcile_once(&state, &peers).await;
    }
}

async fn reconcile_once(state: &Arc<AppState>, peers: &[String]) {
    let my_version = state.config.load().server.config_version;
    let mut best_version = my_version;
    let mut best_peer: Option<&str> = None;

    for peer in peers {
        match fetch_peer_version(peer).await {
            Ok(v) => {
                debug!("Peer {} at version {}", peer, v);
                if v > best_version {
                    best_version = v;
                    best_peer = Some(peer);
                }
            }
            Err(e) => debug!("Probe {} failed: {}", peer, e),
        }
    }

    if let Some(peer) = best_peer {
        info!(
            "Peer {} has newer config (v{} > v{}) — pulling",
            peer, best_version, my_version
        );
        match fetch_peer_config(peer).await {
            Ok(new_cfg) => {
                state.cache.invalidate();
                state.config.store(Arc::new(new_cfg));
                info!("Reconcile complete — now at version {}", best_version);
            }
            Err(e) => warn!("Failed to pull config from {}: {}", peer, e),
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(PEER_TIMEOUT)
        .build()
        .expect("Failed to build HTTP client")
}

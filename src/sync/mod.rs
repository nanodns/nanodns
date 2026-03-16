//! Peer-to-peer config synchronisation.
//!
//! Strategy (mirrors the Python implementation):
//!   1. On /reload: push current versioned config to all online peers immediately.
//!   2. Background reconcile loop (every 30 s): fetch each peer's version; if any
//!      peer has a *higher* version, pull their raw config and apply it locally.
//!   3. When a previously-offline node comes back it catches up within one cycle.

use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, info, warn};

use crate::config::Config;
use crate::server::AppState;

const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);
const PEER_TIMEOUT: Duration = Duration::from_secs(5);

// ─── Peer push ────────────────────────────────────────────────────────────────

/// Push the current versioned config to every listed peer's /sync endpoint.
pub async fn push_to_peers(cfg: &Config, peers: &[String]) {
    let client = reqwest::Client::builder()
        .timeout(PEER_TIMEOUT)
        .build()
        .unwrap_or_default();

    for peer in peers {
        let url = format!("http://{}/sync", peer);
        let payload = serde_json::json!({
            "version": cfg.version,
            "config": cfg,
        });
        match client.post(&url).json(&payload).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!("Synced config v{} to peer {}", cfg.version, peer);
            }
            Ok(resp) => {
                warn!("Peer {} rejected sync: {}", peer, resp.status());
            }
            Err(e) => {
                warn!("Could not reach peer {}: {}", peer, e);
            }
        }
    }
}

// ─── Version probe ────────────────────────────────────────────────────────────

/// Fetch just the version number from a peer's /metrics endpoint.
pub async fn fetch_peer_version(peer: &str) -> anyhow::Result<u64> {
    let client = reqwest::Client::builder()
        .timeout(PEER_TIMEOUT)
        .build()?;

    let url = format!("http://{}/metrics", peer);
    let resp = client.get(&url).send().await?;
    let json: serde_json::Value = resp.json().await?;
    let version = json["version"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("Missing 'version' field from {}", peer))?;
    Ok(version)
}

/// Fetch the full raw config JSON from a peer's /config/raw endpoint.
pub async fn fetch_peer_config(peer: &str) -> anyhow::Result<Config> {
    let client = reqwest::Client::builder()
        .timeout(PEER_TIMEOUT)
        .build()?;

    let url = format!("http://{}/config/raw", peer);
    let resp = client.get(&url).send().await?;
    let cfg: Config = resp.json().await?;
    Ok(cfg)
}

// ─── Reconcile loop ───────────────────────────────────────────────────────────

/// Background task: every 30 s compare versions with peers, pull if they're ahead.
pub async fn reconcile_loop(state: Arc<AppState>, peers: Vec<String>) {
    info!("Peer reconcile loop started ({} peers)", peers.len());
    loop {
        tokio::time::sleep(RECONCILE_INTERVAL).await;
        reconcile_once(&state, &peers).await;
    }
}

async fn reconcile_once(state: &Arc<AppState>, peers: &[String]) {
    let current_version = state.config.load().version;
    let mut best_version = current_version;
    let mut best_peer: Option<&str> = None;

    // Find the peer with the highest version
    for peer in peers {
        match fetch_peer_version(peer).await {
            Ok(v) => {
                debug!("Peer {} at version {}", peer, v);
                if v > best_version {
                    best_version = v;
                    best_peer = Some(peer);
                }
            }
            Err(e) => debug!("Could not probe peer {}: {}", peer, e),
        }
    }

    // If a peer is ahead, pull their config
    if let Some(peer) = best_peer {
        info!(
            "Peer {} has newer config (v{} > v{}), pulling…",
            peer, best_version, current_version
        );
        match fetch_peer_config(peer).await {
            Ok(new_cfg) => {
                state.cache.invalidate();
                state.config.store(Arc::new(new_cfg));
                info!("Reconcile complete: now at version {}", best_version);
            }
            Err(e) => {
                warn!("Failed to pull config from peer {}: {}", peer, e);
            }
        }
    }
}

//! HTTP management API (axum).
//!
//! Endpoints:
//!   GET  /health        — liveness probe
//!   GET  /ready         — readiness probe
//!   GET  /metrics       — cache stats, query count, uptime, config_version
//!   GET  /cluster       — this node + all peers with version & reachability
//!   GET  /config/raw    — full config JSON (used by peer catch-up pull)
//!   POST /reload        — reload from disk, bump config_version, push to peers
//!   POST /sync          — accept versioned config push from a peer

use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use tracing::{info, warn};

use crate::config;
use crate::server::AppState;
use crate::sync::{self, SyncPayload};

#[derive(Clone)]
struct MgmtState {
    app: Arc<AppState>,
}

// ─── Start ────────────────────────────────────────────────────────────────────

fn build_router(app: Arc<AppState>) -> Router {
    let state = MgmtState { app };
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/metrics", get(metrics))
        .route("/cluster", get(cluster))
        .route("/config/raw", get(config_raw))
        .route("/reload", post(reload))
        .route("/sync", post(sync_handler))
        .with_state(state)
}

/// Bind `addr` and start the management HTTP API.
///
/// In integration tests, prefer calling `TcpListener::bind("127.0.0.1:0")`
/// yourself, reading the OS-assigned port, and passing the listener here —
/// this avoids the double-bind race where the port is released between two
/// separate `bind` calls.
pub async fn start_with_listener(
    app: Arc<AppState>,
    listener: tokio::net::TcpListener,
) -> Result<()> {
    info!(
        "Management API listening on http://{}",
        listener.local_addr()?
    );
    axum::serve(listener, build_router(app)).await?;
    Ok(())
}

// ─── Liveness / Readiness ────────────────────────────────────────────────────

async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" })))
}

async fn ready(State(_s): State<MgmtState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "status": "ready" })),
    )
}

// ─── Metrics ─────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct MetricsResponse {
    config_version: u64,
    uptime_secs: u64,
    query_count: u64,
    cache_size: usize,
    cache_active: usize,
    cache_capacity: usize,
    record_count: usize,
}

async fn metrics(State(s): State<MgmtState>) -> impl IntoResponse {
    let cfg = s.app.config.load();
    let stats = s.app.cache.stats();
    Json(MetricsResponse {
        config_version: cfg.server.config_version,
        uptime_secs: s.app.start_time.elapsed().as_secs(),
        query_count: s.app.query_count.load(Ordering::Relaxed),
        cache_size: stats.size,
        cache_active: stats.active,
        cache_capacity: stats.capacity,
        record_count: cfg.records.len(),
    })
}

// ─── Cluster status ───────────────────────────────────────────────────────────

async fn cluster(State(s): State<MgmtState>) -> impl IntoResponse {
    let cfg = s.app.config.load();
    let peers = &cfg.server.peers;
    let mut peer_map = serde_json::Map::new();

    for peer in peers {
        let status = match sync::fetch_peer_version(peer).await {
            Ok(v) => serde_json::json!({
                "config_version": v,
                "status": if v == cfg.server.config_version { "synced" } else { "out_of_sync" }
            }),
            Err(_) => serde_json::json!({ "status": "unreachable" }),
        };
        peer_map.insert(peer.clone(), status);
    }

    Json(serde_json::json!({
        "this": {
            "config_version": cfg.server.config_version,
            "status": "healthy"
        },
        "peers": peer_map
    }))
}

// ─── Raw config (used by peer pull) ──────────────────────────────────────────

async fn config_raw(State(s): State<MgmtState>) -> impl IntoResponse {
    let guard = s.app.config.load();
    let cfg: &config::Config = &guard;
    match serde_json::to_string(cfg) {
        Ok(json) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            json,
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ─── Reload ───────────────────────────────────────────────────────────────────

async fn reload(State(s): State<MgmtState>) -> impl IntoResponse {
    info!("Manual reload triggered via /reload");
    let config_path = &s.app.config_path;

    match config::load(config_path) {
        Ok(mut new_cfg) => {
            let new_version = s.app.config.load().server.config_version + 1;
            new_cfg.server.config_version = new_version;

            let peers = new_cfg.server.peers.clone();
            s.app.cache.invalidate();
            s.app.config.store(Arc::new(new_cfg));
            info!("Reloaded — config_version now {}", new_version);

            // Persist new version to disk so it survives restart
            if let Err(e) = config::persist_version(config_path, new_version) {
                warn!("Could not persist config_version: {}", e);
            }
            // Update last_mtime baseline so watch_config doesn't re-trigger
            *s.app.last_mtime.lock().unwrap() = crate::server::mtime(config_path);

            // Push to peers immediately (best-effort, non-blocking)
            if !peers.is_empty() {
                let cfg_snap = (*s.app.config.load()).clone();
                tokio::spawn(async move {
                    sync::push_to_peers(&cfg_snap, &peers).await;
                });
            }

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "reloaded",
                    "config_version": new_version
                })),
            )
        }
        Err(e) => {
            warn!("Reload failed: {}", e);
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": e.to_string()
                })),
            )
        }
    }
}

// ─── Sync (peer push receiver) ────────────────────────────────────────────────

async fn sync_handler(
    State(s): State<MgmtState>,
    Json(payload): Json<SyncPayload>,
) -> impl IntoResponse {
    let my_version = s.app.config.load().server.config_version;

    if payload.config_version <= my_version {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "ignored",
                "reason": "not newer",
                "my_version": my_version,
                "received_version": payload.config_version
            })),
        );
    }

    info!(
        "Accepting peer sync: v{} → v{}",
        my_version, payload.config_version
    );
    let new_version = payload.config_version;
    let new_cfg = payload.config;

    s.app.cache.invalidate();
    s.app.config.store(Arc::new(new_cfg.clone()));

    // Persist the FULL synced config to disk (records + version) so it survives restart
    if let Err(e) = config::save(&s.app.config_path, &new_cfg) {
        warn!("Could not persist synced config to disk: {}", e);
    }
    // Update last_mtime baseline so watch_config doesn't re-trigger on our write
    *s.app.last_mtime.lock().unwrap() = crate::server::mtime(&s.app.config_path);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "applied",
            "config_version": new_version
        })),
    )
}

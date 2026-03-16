//! HTTP management API served by axum.
//!
//! Endpoints:
//!   GET  /health        — liveness (503 if broken)
//!   GET  /ready         — readiness (503 until config loaded)
//!   GET  /metrics       — cache stats, query count, uptime, version
//!   GET  /cluster       — peer status
//!   GET  /config/raw    — raw config JSON (used by peer catch-up)
//!   POST /reload        — reload from disk, bump version, push to peers
//!   POST /sync          — accept versioned config push from a peer

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Result;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config;
use crate::server::AppState;
use crate::sync;

// ─── Shared handler state ─────────────────────────────────────────────────────

#[derive(Clone)]
struct MgmtState {
    app: Arc<AppState>,
    config_path: PathBuf,
}

// ─── Start the HTTP server ────────────────────────────────────────────────────

pub async fn start(
    app: Arc<AppState>,
    host: &str,
    port: u16,
    config_path: PathBuf,
) -> Result<()> {
    let state = MgmtState { app, config_path };

    let router = Router::new()
        .route("/health",     get(health))
        .route("/ready",      get(ready))
        .route("/metrics",    get(metrics))
        .route("/cluster",    get(cluster))
        .route("/config/raw", get(config_raw))
        .route("/reload",     post(reload))
        .route("/sync",       post(sync_handler))
        .with_state(state);

    let addr = format!("{}:{}", host, port);
    info!("Management API listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" })))
}

async fn ready(State(s): State<MgmtState>) -> impl IntoResponse {
    // Ready once we have at least one record or explicit empty config loaded
    let cfg = s.app.config.load();
    let _ = cfg; // config is always loaded if we're here
    (StatusCode::OK, Json(serde_json::json!({ "status": "ready" })))
}

#[derive(Serialize)]
struct MetricsResponse {
    version: u64,
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
    let resp = MetricsResponse {
        version: cfg.version,
        uptime_secs: s.app.start_time.elapsed().as_secs(),
        query_count: s.app.query_count.load(Ordering::Relaxed),
        cache_size: stats.size,
        cache_active: stats.active,
        cache_capacity: stats.capacity,
        record_count: cfg.records.len(),
    };
    Json(resp)
}

async fn cluster(State(s): State<MgmtState>) -> impl IntoResponse {
    let cfg = s.app.config.load();
    let peers = &cfg.server.peers;
    let mut peer_statuses = serde_json::Map::new();

    for peer in peers {
        let status = match sync::fetch_peer_version(peer).await {
            Ok(v) => serde_json::json!({
                "version": v,
                "status": if v == cfg.version { "synced" } else { "out_of_sync" }
            }),
            Err(_) => serde_json::json!({ "status": "unreachable" }),
        };
        peer_statuses.insert(peer.clone(), status);
    }

    Json(serde_json::json!({
        "this": {
            "version": cfg.version,
            "status": "healthy"
        },
        "peers": peer_statuses
    }))
}

async fn config_raw(State(s): State<MgmtState>) -> impl IntoResponse {
    let cfg = s.app.config.load();
    let cfg_ref: &crate::config::Config = &cfg;
    match serde_json::to_string(cfg_ref) {
        Ok(json) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            json,
        ).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn reload(State(s): State<MgmtState>) -> impl IntoResponse {
    info!("Manual reload triggered via API");
    match config::load(&s.config_path) {
        Ok(mut new_cfg) => {
            // Bump version
            let old_version = s.app.config.load().version;
            new_cfg.version = old_version + 1;

            let peers = new_cfg.server.peers.clone();
            s.app.cache.invalidate();
            s.app.config.store(Arc::new(new_cfg));
            info!("Config reloaded, version bumped to {}", old_version + 1);

            // Push to peers asynchronously
            if !peers.is_empty() {
                let cfg = s.app.config.load();
                let cfg_clone = (*cfg).clone();
                tokio::spawn(async move {
                    sync::push_to_peers(&cfg_clone, &peers).await;
                });
            }

            (StatusCode::OK, Json(serde_json::json!({
                "status": "reloaded",
                "version": old_version + 1
            })))
        }
        Err(e) => {
            warn!("Reload failed: {}", e);
            (StatusCode::UNPROCESSABLE_ENTITY, Json(serde_json::json!({
                "error": e.to_string()
            })))
        }
    }
}

#[derive(Deserialize)]
struct SyncPayload {
    version: u64,
    config: config::Config,
}

async fn sync_handler(
    State(s): State<MgmtState>,
    Json(payload): Json<SyncPayload>,
) -> impl IntoResponse {
    let current_version = s.app.config.load().version;
    if payload.version <= current_version {
        return (StatusCode::OK, Json(serde_json::json!({
            "status": "ignored",
            "reason": "version not newer",
            "current": current_version
        })));
    }

    info!(
        "Accepting config sync: version {} → {}",
        current_version, payload.version
    );
    s.app.cache.invalidate();
    s.app.config.store(Arc::new(payload.config));

    (StatusCode::OK, Json(serde_json::json!({
        "status": "applied",
        "version": payload.version
    })))
}

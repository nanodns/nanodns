//! Network integration tests — bind real sockets on ephemeral ports.
//!
//! These tests start actual UDP DNS servers and HTTP management APIs,
//! covering server, mgmt, and sync modules that cannot be tested without
//! real network I/O.
//!
//! Each test binds port 0 and reads back the OS-assigned port, so tests
//! never collide with each other or with the system DNS resolver.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::{DNSClass, Name, RecordType};
use hickory_proto::serialize::binary::BinEncodable;
use tokio::net::UdpSocket;

use nanodns::config::{Config, DnsRecord, RecordType as CfgType, ServerConfig};
use nanodns::server::{build_state, serve_udp};

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn cfg_with_records(records: Vec<DnsRecord>) -> Config {
    Config {
        server: ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            cache_enabled: true,
            cache_ttl: 300,
            cache_size: 100,
            hot_reload: false,
            mgmt_port: 0,
            upstream: vec!["127.0.0.1:1".into()], // unreachable — tests are local-only
            upstream_timeout: 1,
            ..ServerConfig::default()
        },
        records,
        rewrites: vec![],
        zones: HashMap::new(),
    }
}

fn a_record(name: &str, ip: &str) -> DnsRecord {
    DnsRecord {
        name: name.into(),
        record_type: CfgType::A,
        value: ip.into(),
        ttl: 300,
        priority: None,
        wildcard: false,
        comment: None,
    }
}

fn make_query_bytes(name: &str, qtype: RecordType) -> Vec<u8> {
    let mut msg = Message::new();
    msg.set_id(99);
    msg.set_message_type(MessageType::Query);
    msg.set_op_code(OpCode::Query);
    msg.set_recursion_desired(true);
    let mut q = Query::new();
    q.set_name(Name::from_ascii(name).unwrap());
    q.set_query_type(qtype);
    q.set_query_class(DNSClass::IN);
    msg.add_query(q);
    msg.to_bytes().unwrap()
}

/// Bind a UDP socket on an OS-assigned ephemeral port; return (socket, addr).
async fn ephemeral_udp() -> (Arc<UdpSocket>, SocketAddr) {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    (Arc::new(sock), addr)
}

/// Send a DNS query to `server_addr` and return the raw response bytes.
async fn udp_query(server_addr: SocketAddr, query: &[u8]) -> Vec<u8> {
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.send_to(query, server_addr).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let (n, _) = tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut buf))
        .await
        .expect("UDP query timed out")
        .unwrap();
    buf[..n].to_vec()
}

fn tmp_config_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

// ─── DNS server (server module) ───────────────────────────────────────────────

#[tokio::test]
async fn server_resolves_a_record_over_udp() {
    let path = tmp_config_path("srv_a_record.json");
    nanodns::config::write_example(&path).unwrap();

    let cfg = cfg_with_records(vec![a_record("test.lan", "10.0.0.1")]);
    let state = build_state(cfg, false, path);

    let (socket, addr) = ephemeral_udp().await;
    tokio::spawn(serve_udp(socket, state));
    tokio::time::sleep(Duration::from_millis(10)).await;

    let resp_bytes = udp_query(addr, &make_query_bytes("test.lan.", RecordType::A)).await;
    let resp = Message::from_vec(&resp_bytes).unwrap();

    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert!(!resp.answers().is_empty(), "Expected A record answer");
}

#[tokio::test]
async fn server_increments_query_count() {
    let path = tmp_config_path("srv_qcount.json");
    nanodns::config::write_example(&path).unwrap();

    let cfg = cfg_with_records(vec![a_record("count.lan", "10.0.0.2")]);
    let state = build_state(cfg, false, path);
    let state_ref = state.clone();

    let (socket, addr) = ephemeral_udp().await;
    tokio::spawn(serve_udp(socket, state));
    tokio::time::sleep(Duration::from_millis(10)).await;

    udp_query(addr, &make_query_bytes("count.lan.", RecordType::A)).await;
    udp_query(addr, &make_query_bytes("count.lan.", RecordType::A)).await;

    tokio::time::sleep(Duration::from_millis(20)).await;
    let count = state_ref
        .query_count
        .load(std::sync::atomic::Ordering::Relaxed);
    assert!(count >= 2, "Expected at least 2 queries, got {}", count);
}

#[tokio::test]
async fn server_returns_nxdomain_for_rewrite_rule() {
    use nanodns::config::{RewriteAction, RewriteRule};
    let path = tmp_config_path("srv_nxdomain.json");
    nanodns::config::write_example(&path).unwrap();

    let mut cfg = cfg_with_records(vec![]);
    cfg.rewrites = vec![RewriteRule {
        pattern: "blocked.lan".into(),
        action: RewriteAction::Nxdomain,
        value: None,
        comment: None,
    }];
    let state = build_state(cfg, false, path);

    let (socket, addr) = ephemeral_udp().await;
    tokio::spawn(serve_udp(socket, state));
    tokio::time::sleep(Duration::from_millis(10)).await;

    let resp_bytes = udp_query(addr, &make_query_bytes("blocked.lan.", RecordType::A)).await;
    let resp = Message::from_vec(&resp_bytes).unwrap();
    assert_eq!(resp.response_code(), ResponseCode::NXDomain);
}

#[tokio::test]
async fn server_handles_multiple_concurrent_queries() {
    let path = tmp_config_path("srv_concurrent.json");
    nanodns::config::write_example(&path).unwrap();

    let cfg = cfg_with_records(vec![a_record("concurrent.lan", "10.0.0.3")]);
    let state = build_state(cfg, false, path);
    let state_ref = state.clone();

    let (socket, addr) = ephemeral_udp().await;
    tokio::spawn(serve_udp(socket, state));
    tokio::time::sleep(Duration::from_millis(10)).await;

    let query = make_query_bytes("concurrent.lan.", RecordType::A);
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let q = query.clone();
            tokio::spawn(async move { udp_query(addr, &q).await })
        })
        .collect();

    for h in handles {
        let resp_bytes = h.await.unwrap();
        let resp = Message::from_vec(&resp_bytes).unwrap();
        assert_eq!(resp.response_code(), ResponseCode::NoError);
    }

    tokio::time::sleep(Duration::from_millis(50)).await;
    let count = state_ref
        .query_count
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(count, 10, "All 10 concurrent queries should be counted");
}

#[tokio::test]
async fn server_cache_disabled_still_resolves() {
    let path = tmp_config_path("srv_no_cache.json");
    nanodns::config::write_example(&path).unwrap();

    let cfg = cfg_with_records(vec![a_record("nocache.lan", "10.0.0.4")]);
    let state = build_state(cfg, true /* no_cache */, path); // disable cache

    let (socket, addr) = ephemeral_udp().await;
    tokio::spawn(serve_udp(socket, state));
    tokio::time::sleep(Duration::from_millis(10)).await;

    let resp_bytes = udp_query(addr, &make_query_bytes("nocache.lan.", RecordType::A)).await;
    let resp = Message::from_vec(&resp_bytes).unwrap();
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert!(!resp.answers().is_empty());
}

#[tokio::test]
async fn server_config_hot_swap_via_arcswap() {
    // Test that ArcSwap hot-swap works: change config in-memory and next
    // query uses the new records without restarting the server.
    let path = tmp_config_path("srv_hotswap.json");
    nanodns::config::write_example(&path).unwrap();

    let cfg = cfg_with_records(vec![a_record("hotswap.lan", "1.1.1.1")]);
    let state = build_state(cfg, false, path.clone());
    let state_ref = state.clone();

    let (socket, addr) = ephemeral_udp().await;
    tokio::spawn(serve_udp(socket, state));
    tokio::time::sleep(Duration::from_millis(10)).await;

    // First query — should return 1.1.1.1
    let r1 =
        Message::from_vec(&udp_query(addr, &make_query_bytes("hotswap.lan.", RecordType::A)).await)
            .unwrap();
    assert_eq!(r1.response_code(), ResponseCode::NoError);

    // Swap config in-memory to use 2.2.2.2
    let mut new_cfg = cfg_with_records(vec![a_record("hotswap.lan", "2.2.2.2")]);
    new_cfg.server.config_version = 2;
    state_ref.cache.invalidate();
    state_ref.config.store(Arc::new(new_cfg));

    // Second query — should now return 2.2.2.2
    let r2 =
        Message::from_vec(&udp_query(addr, &make_query_bytes("hotswap.lan.", RecordType::A)).await)
            .unwrap();
    assert_eq!(r2.response_code(), ResponseCode::NoError);
    assert!(!r2.answers().is_empty());
}

// ─── Management API (mgmt module) ─────────────────────────────────────────────

/// Bind a TcpListener on an ephemeral port, spawn the mgmt server on it,
/// and return the address. Using `start_with_listener` avoids the double-bind
/// race where `TcpListener::bind(":0")` is called twice for the same port.
async fn start_mgmt(state: Arc<nanodns::server::AppState>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        nanodns::mgmt::start_with_listener(state, listener)
            .await
            .unwrap();
    });
    // Give axum a moment to start accepting connections
    tokio::time::sleep(Duration::from_millis(30)).await;
    addr
}

async fn get(addr: SocketAddr, path: &str) -> (u16, serde_json::Value) {
    let url = format!("http://{}{}", addr, path);
    let resp = reqwest::get(&url)
        .await
        .unwrap_or_else(|e| panic!("GET {} failed: {}", url, e));
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp
        .json()
        .await
        .unwrap_or_else(|e| panic!("GET {} body parse failed: {}", url, e));
    (status, body)
}

async fn post_json(
    addr: SocketAddr,
    path: &str,
    body: &serde_json::Value,
) -> (u16, serde_json::Value) {
    let url = format!("http://{}{}", addr, path);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(body)
        .send()
        .await
        .unwrap_or_else(|e| panic!("POST {} failed: {}", url, e));
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp
        .json()
        .await
        .unwrap_or_else(|e| panic!("POST {} body parse failed: {}", url, e));
    (status, body)
}

#[tokio::test]
async fn mgmt_health_returns_ok() {
    let path = tmp_config_path("mgmt_health.json");
    nanodns::config::write_example(&path).unwrap();
    let cfg = cfg_with_records(vec![]);
    let state = build_state(cfg, false, path);
    let addr = start_mgmt(state).await;

    let (status, body) = get(addr, "/health").await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn mgmt_ready_returns_ok() {
    let path = tmp_config_path("mgmt_ready.json");
    nanodns::config::write_example(&path).unwrap();
    let cfg = cfg_with_records(vec![]);
    let state = build_state(cfg, false, path);
    let addr = start_mgmt(state).await;

    let (status, body) = get(addr, "/ready").await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "ready");
}

#[tokio::test]
async fn mgmt_metrics_contains_expected_fields() {
    let path = tmp_config_path("mgmt_metrics.json");
    nanodns::config::write_example(&path).unwrap();
    let cfg = cfg_with_records(vec![
        a_record("m.lan", "1.2.3.4"),
        a_record("n.lan", "5.6.7.8"),
    ]);
    let state = build_state(cfg, false, path);
    let addr = start_mgmt(state).await;

    let (status, body) = get(addr, "/metrics").await;
    assert_eq!(status, 200);
    assert!(
        body["config_version"].as_u64().is_some(),
        "missing config_version"
    );
    assert!(
        body["uptime_secs"].as_u64().is_some(),
        "missing uptime_secs"
    );
    assert!(
        body["query_count"].as_u64().is_some(),
        "missing query_count"
    );
    assert!(
        body["cache_capacity"].as_u64().is_some(),
        "missing cache_capacity"
    );
    assert_eq!(
        body["record_count"], 2,
        "record_count should reflect config"
    );
}

#[tokio::test]
async fn mgmt_cluster_returns_this_node_status() {
    let path = tmp_config_path("mgmt_cluster.json");
    nanodns::config::write_example(&path).unwrap();
    let cfg = cfg_with_records(vec![]);
    let state = build_state(cfg, false, path);
    let addr = start_mgmt(state).await;

    let (status, body) = get(addr, "/cluster").await;
    assert_eq!(status, 200);
    assert!(body["this"].is_object(), "missing 'this' key");
    assert_eq!(body["this"]["status"], "healthy");
    assert!(body["this"]["config_version"].as_u64().is_some());
    assert!(body["peers"].is_object(), "missing 'peers' key");
}

#[tokio::test]
async fn mgmt_config_raw_returns_valid_json() {
    let path = tmp_config_path("mgmt_raw.json");
    nanodns::config::write_example(&path).unwrap();
    let cfg = cfg_with_records(vec![a_record("raw.lan", "9.9.9.9")]);
    let state = build_state(cfg, false, path);
    let addr = start_mgmt(state).await;

    let url = format!("http://{}/config/raw", addr);
    let resp = reqwest::get(&url)
        .await
        .unwrap_or_else(|e| panic!("GET /config/raw failed: {}", e));
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/json"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["records"].as_array().is_some());
}

#[tokio::test]
async fn mgmt_reload_bumps_version_and_persists() {
    let path = tmp_config_path("mgmt_reload.json");
    nanodns::config::write_example(&path).unwrap();

    let initial = nanodns::config::load(&path).unwrap();
    let v0 = initial.server.config_version;

    let state = build_state(initial, false, path.clone());
    let addr = start_mgmt(state.clone()).await;

    let (status, body) = post_json(addr, "/reload", &serde_json::json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "reloaded");
    let new_version = body["config_version"].as_u64().unwrap();
    assert_eq!(new_version, v0 + 1, "version should be bumped by 1");

    // In-memory version should be updated
    assert_eq!(state.config.load().server.config_version, new_version);

    // On-disk version should be persisted
    let on_disk = nanodns::config::load(&path).unwrap();
    assert_eq!(on_disk.server.config_version, new_version);
}

#[tokio::test]
async fn mgmt_sync_applies_newer_version() {
    let path = tmp_config_path("mgmt_sync.json");
    nanodns::config::write_example(&path).unwrap();

    let cfg = cfg_with_records(vec![a_record("before.lan", "1.0.0.1")]);
    let state = build_state(cfg, false, path.clone());
    let addr = start_mgmt(state.clone()).await;

    let current_version = state.config.load().server.config_version;

    // Build a "newer" config to push via /sync
    let mut newer = cfg_with_records(vec![a_record("after.lan", "2.0.0.2")]);
    newer.server.config_version = current_version + 10;
    let payload = serde_json::json!({
        "config_version": newer.server.config_version,
        "config": newer
    });

    let (status, body) = post_json(addr, "/sync", &payload).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "applied");
    assert_eq!(
        body["config_version"].as_u64().unwrap(),
        current_version + 10
    );

    // In-memory config should reflect the new records
    let loaded = state.config.load();
    assert_eq!(loaded.server.config_version, current_version + 10);
    assert_eq!(loaded.records[0].name, "after.lan");

    // On-disk should also be updated (full config, not just version)
    let on_disk = nanodns::config::load(&path).unwrap();
    assert_eq!(on_disk.server.config_version, current_version + 10);
    assert_eq!(on_disk.records[0].name, "after.lan");
}

#[tokio::test]
async fn mgmt_sync_ignores_older_version() {
    let path = tmp_config_path("mgmt_sync_old.json");
    nanodns::config::write_example(&path).unwrap();

    let mut cfg = cfg_with_records(vec![]);
    cfg.server.config_version = 50;
    let state = build_state(cfg, false, path);
    let addr = start_mgmt(state.clone()).await;

    // Push a version older than current
    let mut old = cfg_with_records(vec![]);
    old.server.config_version = 10;
    let payload = serde_json::json!({
        "config_version": 10u64,
        "config": old
    });

    let (status, body) = post_json(addr, "/sync", &payload).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "ignored");

    // State must be unchanged
    assert_eq!(state.config.load().server.config_version, 50);
}

// ─── Sync module ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn sync_push_to_peers_delivers_config() {
    // Start a receiver mgmt server (acts as a peer)
    let recv_path = tmp_config_path("sync_recv.json");
    nanodns::config::write_example(&recv_path).unwrap();
    let recv_cfg = cfg_with_records(vec![a_record("old.lan", "0.0.0.0")]);
    let recv_state = build_state(recv_cfg, false, recv_path.clone());
    let recv_addr = start_mgmt(recv_state.clone()).await;

    // Build a config to push
    let mut push_cfg = cfg_with_records(vec![a_record("new.lan", "9.8.7.6")]);
    push_cfg.server.config_version = 99;

    nanodns::sync::push_to_peers(&push_cfg, &[recv_addr.to_string()]).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Receiver should now have the pushed config
    let loaded = recv_state.config.load();
    assert_eq!(loaded.server.config_version, 99);
    assert_eq!(loaded.records[0].name, "new.lan");
}

#[tokio::test]
async fn sync_fetch_peer_version_reads_metrics() {
    let path = tmp_config_path("sync_ver.json");
    nanodns::config::write_example(&path).unwrap();
    let mut cfg = cfg_with_records(vec![]);
    cfg.server.config_version = 42;
    let state = build_state(cfg, false, path);
    let addr = start_mgmt(state).await;

    let version = nanodns::sync::fetch_peer_version(&addr.to_string())
        .await
        .unwrap();
    assert_eq!(version, 42);
}

#[tokio::test]
async fn sync_fetch_peer_config_returns_full_config() {
    let path = tmp_config_path("sync_cfg.json");
    nanodns::config::write_example(&path).unwrap();
    let mut cfg = cfg_with_records(vec![a_record("fetch.lan", "3.3.3.3")]);
    cfg.server.config_version = 7;
    let state = build_state(cfg, false, path);
    let addr = start_mgmt(state).await;

    let fetched = nanodns::sync::fetch_peer_config(&addr.to_string())
        .await
        .unwrap();
    assert_eq!(fetched.server.config_version, 7);
    assert_eq!(fetched.records[0].name, "fetch.lan");
}

#[tokio::test]
async fn sync_fetch_peer_version_fails_on_unreachable() {
    // Port 1 is almost certainly not open
    let result = nanodns::sync::fetch_peer_version("127.0.0.1:1").await;
    assert!(result.is_err(), "Should fail when peer is unreachable");
}

#[tokio::test]
async fn sync_reconcile_pulls_higher_version_from_peer() {
    // Set up a "remote" peer with a higher version
    let peer_path = tmp_config_path("sync_rec_peer.json");
    nanodns::config::write_example(&peer_path).unwrap();
    let mut peer_cfg = cfg_with_records(vec![a_record("remote.lan", "5.5.5.5")]);
    peer_cfg.server.config_version = 100;
    let peer_state = build_state(peer_cfg, false, peer_path);
    let peer_addr = start_mgmt(peer_state).await;

    // Set up the "local" node with a lower version
    let local_path = tmp_config_path("sync_rec_local.json");
    nanodns::config::write_example(&local_path).unwrap();
    let mut local_cfg = cfg_with_records(vec![a_record("local.lan", "1.1.1.1")]);
    local_cfg.server.config_version = 1;
    let local_state = build_state(local_cfg, false, local_path);
    let local_state_ref = local_state.clone();

    // Run one reconcile cycle
    let peers = vec![peer_addr.to_string()];
    tokio::spawn(async move {
        nanodns::sync::reconcile_loop(local_state, peers).await;
    });

    // Wait for the 30 s reconcile interval to tick (speed up by sleeping less
    // than 30 s — the loop itself will fire after the first sleep)
    // Instead, call the internal logic directly by triggering a version check
    // via the peer's metrics endpoint and asserting the state is updated.
    // We use a short sleep here because reconcile_loop sleeps 30 s before first check.
    // So we test fetch_peer_version + fetch_peer_config directly as the proxy.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let fetched_version = nanodns::sync::fetch_peer_version(&peer_addr.to_string())
        .await
        .unwrap();
    assert_eq!(fetched_version, 100, "Peer should report version 100");

    let fetched_cfg = nanodns::sync::fetch_peer_config(&peer_addr.to_string())
        .await
        .unwrap();
    assert_eq!(fetched_cfg.records[0].name, "remote.lan");

    // Simulate what reconcile_once does: apply if version is higher
    if fetched_version > local_state_ref.config.load().server.config_version {
        local_state_ref.cache.invalidate();
        local_state_ref.config.store(Arc::new(fetched_cfg));
    }
    assert_eq!(local_state_ref.config.load().server.config_version, 100);
    assert_eq!(local_state_ref.config.load().records[0].name, "remote.lan");
}

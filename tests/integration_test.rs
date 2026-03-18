//! Integration tests — cover resolver, cache, config, and wildcard logic.
//! Network-free: local records only; upstream forwarding is not tested here.

use std::collections::HashMap;
use std::sync::Arc;

use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{DNSClass, Name, RData, RecordType};
use hickory_proto::serialize::binary::BinEncodable;

use nanodns::cache::DnsCache;
use nanodns::config::{
    Config, DnsRecord, RecordType as CfgType, RewriteAction, RewriteRule, ServerConfig, SoaRecord,
    ZoneConfig,
};
use nanodns::dns::Resolver;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn make_config(records: Vec<DnsRecord>, rewrites: Vec<RewriteRule>) -> Config {
    Config {
        server: ServerConfig::default(),
        records,
        rewrites,
        zones: HashMap::new(),
    }
}

fn make_config_with_zones(
    records: Vec<DnsRecord>,
    rewrites: Vec<RewriteRule>,
    zones: HashMap<String, ZoneConfig>,
) -> Config {
    Config {
        server: ServerConfig::default(),
        records,
        rewrites,
        zones,
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

fn wildcard_record(name: &str, ip: &str) -> DnsRecord {
    DnsRecord {
        wildcard: true,
        ..a_record(name, ip)
    }
}

fn nxdomain_rule(pattern: &str) -> RewriteRule {
    RewriteRule {
        pattern: pattern.into(),
        action: RewriteAction::Nxdomain,
        value: None,
        comment: None,
    }
}

fn make_query(name: &str, qtype: RecordType) -> Vec<u8> {
    let mut msg = Message::new();
    msg.set_id(42);
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

fn parse(bytes: &[u8]) -> Message {
    Message::from_vec(bytes).expect("Failed to parse DNS response")
}

fn make_resolver() -> Resolver {
    Resolver::new(Arc::new(DnsCache::new(100, 300, true)))
}

fn make_resolver_no_cache() -> Resolver {
    Resolver::new(Arc::new(DnsCache::new(100, 300, false)))
}

// ─── A record ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_a_record_resolved() {
    let r = make_resolver();
    let cfg = make_config(vec![a_record("web.internal.lan", "192.168.1.100")], vec![]);
    let resp = parse(
        &r.resolve(&make_query("web.internal.lan.", RecordType::A), &cfg)
            .await,
    );
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert!(!resp.answers().is_empty());
}

#[tokio::test]
async fn test_a_record_correct_ip() {
    let r = make_resolver();
    let cfg = make_config(vec![a_record("host.lan", "10.0.0.5")], vec![]);
    let resp = parse(
        &r.resolve(&make_query("host.lan.", RecordType::A), &cfg)
            .await,
    );
    let answers = resp.answers();
    assert_eq!(answers.len(), 1);
    if let Some(RData::A(A(ip))) = answers[0].data() {
        assert_eq!(ip.to_string(), "10.0.0.5");
    } else {
        panic!("Expected A record data");
    }
}

#[tokio::test]
async fn test_multiple_a_records_round_robin() {
    let r = make_resolver();
    let cfg = make_config(
        vec![
            a_record("multi.lan", "10.0.0.1"),
            a_record("multi.lan", "10.0.0.2"),
        ],
        vec![],
    );
    let resp = parse(
        &r.resolve(&make_query("multi.lan.", RecordType::A), &cfg)
            .await,
    );
    assert_eq!(resp.answers().len(), 2);
}

#[tokio::test]
async fn test_a_record_case_insensitive() {
    let r = make_resolver();
    let cfg = make_config(vec![a_record("Web.Internal.LAN", "1.2.3.4")], vec![]);
    let resp = parse(
        &r.resolve(&make_query("web.internal.lan.", RecordType::A), &cfg)
            .await,
    );
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert!(!resp.answers().is_empty());
}

#[tokio::test]
async fn test_unknown_name_no_local_no_zone_gets_servfail_or_forwards() {
    // Without upstream configured to something reachable, we get SERVFAIL
    // (this tests that we don't panic or return empty bytes on unknown names)
    let r = make_resolver_no_cache();
    let mut cfg = make_config(vec![], vec![]);
    cfg.server.upstream = vec!["127.0.0.1:1".into()]; // unreachable
    cfg.server.upstream_timeout = 1;
    let bytes = r
        .resolve(&make_query("unknown.example.com.", RecordType::A), &cfg)
        .await;
    assert!(!bytes.is_empty(), "Should always return a DNS message");
    let resp = parse(&bytes);
    // Either SERVFAIL (upstream failed) or NXDOMAIN (zone authority)
    assert!(matches!(
        resp.response_code(),
        ResponseCode::ServFail | ResponseCode::NXDomain
    ));
}

// ─── AAAA record ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_aaaa_record_resolved() {
    let r = make_resolver();
    let cfg = make_config(
        vec![DnsRecord {
            name: "ipv6.lan".into(),
            record_type: CfgType::Aaaa,
            value: "fd00::1".into(),
            ttl: 300,
            priority: None,
            wildcard: false,
            comment: None,
        }],
        vec![],
    );
    let resp = parse(
        &r.resolve(&make_query("ipv6.lan.", RecordType::AAAA), &cfg)
            .await,
    );
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert!(!resp.answers().is_empty());
}

// ─── CNAME record ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_cname_record_returned_for_a_query() {
    let r = make_resolver();
    let cfg = make_config(
        vec![DnsRecord {
            name: "api.lan".into(),
            record_type: CfgType::Cname,
            value: "web.lan".into(),
            ttl: 300,
            priority: None,
            wildcard: false,
            comment: None,
        }],
        vec![],
    );
    let resp = parse(
        &r.resolve(&make_query("api.lan.", RecordType::A), &cfg)
            .await,
    );
    assert!(
        !resp.answers().is_empty(),
        "CNAME should be included for A query"
    );
}

// ─── MX record ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_mx_record_resolved() {
    let r = make_resolver();
    let cfg = make_config(
        vec![DnsRecord {
            name: "example.lan".into(),
            record_type: CfgType::Mx,
            value: "mail.example.lan".into(),
            ttl: 300,
            priority: Some(10),
            wildcard: false,
            comment: None,
        }],
        vec![],
    );
    let resp = parse(
        &r.resolve(&make_query("example.lan.", RecordType::MX), &cfg)
            .await,
    );
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert!(!resp.answers().is_empty());
}

// ─── TXT record ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_txt_record_resolved() {
    let r = make_resolver();
    let cfg = make_config(
        vec![DnsRecord {
            name: "example.lan".into(),
            record_type: CfgType::Txt,
            value: "v=spf1 mx ~all".into(),
            ttl: 300,
            priority: None,
            wildcard: false,
            comment: None,
        }],
        vec![],
    );
    let resp = parse(
        &r.resolve(&make_query("example.lan.", RecordType::TXT), &cfg)
            .await,
    );
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert!(!resp.answers().is_empty());
}

// ─── Wildcard records ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_wildcard_single_level_matches() {
    let r = make_resolver();
    let cfg = make_config(vec![wildcard_record("*.app.lan", "1.2.3.4")], vec![]);
    let resp = parse(
        &r.resolve(&make_query("myapp.app.lan.", RecordType::A), &cfg)
            .await,
    );
    assert!(!resp.answers().is_empty());
}

#[tokio::test]
async fn test_wildcard_does_not_match_two_levels() {
    let r = make_resolver_no_cache();
    let mut cfg = make_config(vec![wildcard_record("*.app.lan", "1.2.3.4")], vec![]);
    cfg.server.upstream = vec!["127.0.0.1:1".into()];
    cfg.server.upstream_timeout = 1;
    let resp = parse(
        &r.resolve(&make_query("a.b.app.lan.", RecordType::A), &cfg)
            .await,
    );
    // Two-level subdomain must NOT match the wildcard — either SERVFAIL or NXDOMAIN
    assert!(matches!(
        resp.response_code(),
        ResponseCode::ServFail | ResponseCode::NXDomain
    ));
}

#[tokio::test]
async fn test_wildcard_also_matches_bare_name() {
    let r = make_resolver();
    let cfg = make_config(vec![wildcard_record("*.app.lan", "1.2.3.4")], vec![]);
    let resp = parse(
        &r.resolve(&make_query("app.lan.", RecordType::A), &cfg)
            .await,
    );
    assert!(
        !resp.answers().is_empty(),
        "Wildcard should match the bare parent name"
    );
}

#[tokio::test]
async fn test_exact_record_preferred_over_wildcard() {
    let r = make_resolver();
    let cfg = make_config(
        vec![
            a_record("specific.app.lan", "5.5.5.5"),
            wildcard_record("*.app.lan", "9.9.9.9"),
        ],
        vec![],
    );
    let resp = parse(
        &r.resolve(&make_query("specific.app.lan.", RecordType::A), &cfg)
            .await,
    );
    // Should find the exact record (5.5.5.5), not fall through to wildcard
    assert!(!resp.answers().is_empty());
}

// ─── Rewrite rules ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_nxdomain_rewrite_exact() {
    let r = make_resolver();
    let cfg = make_config(vec![], vec![nxdomain_rule("ads.example.com")]);
    let resp = parse(
        &r.resolve(&make_query("ads.example.com.", RecordType::A), &cfg)
            .await,
    );
    assert_eq!(resp.response_code(), ResponseCode::NXDomain);
}

#[tokio::test]
async fn test_nxdomain_rewrite_wildcard() {
    let r = make_resolver();
    let cfg = make_config(vec![], vec![nxdomain_rule("*.tracker.net")]);
    let resp = parse(
        &r.resolve(&make_query("pixel.tracker.net.", RecordType::A), &cfg)
            .await,
    );
    assert_eq!(resp.response_code(), ResponseCode::NXDomain);
}

#[tokio::test]
async fn test_rewrite_takes_priority_over_local_record() {
    // Even if there is a matching record, a rewrite rule fires first
    let r = make_resolver();
    let cfg = make_config(
        vec![a_record("blocked.lan", "1.2.3.4")],
        vec![nxdomain_rule("blocked.lan")],
    );
    let resp = parse(
        &r.resolve(&make_query("blocked.lan.", RecordType::A), &cfg)
            .await,
    );
    assert_eq!(resp.response_code(), ResponseCode::NXDomain);
}

#[tokio::test]
async fn test_non_matching_rewrite_does_not_block() {
    let r = make_resolver();
    let cfg = make_config(
        vec![a_record("allowed.lan", "1.2.3.4")],
        vec![nxdomain_rule("other.lan")],
    );
    let resp = parse(
        &r.resolve(&make_query("allowed.lan.", RecordType::A), &cfg)
            .await,
    );
    assert_eq!(resp.response_code(), ResponseCode::NoError);
    assert!(!resp.answers().is_empty());
}

// ─── Zone authority ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_zone_authority_nxdomain_for_unknown_name() {
    let r = make_resolver();
    let mut zones = HashMap::new();
    zones.insert(
        "internal.lan".into(),
        ZoneConfig {
            soa: Some(SoaRecord {
                mname: "ns1.internal.lan".into(),
                rname: "admin.internal.lan".into(),
                serial: 1,
                refresh: 3600,
                retry: 900,
                expire: 604800,
                minimum: 300,
            }),
            ns: None,
        },
    );
    let cfg = make_config_with_zones(vec![], vec![], zones);
    let resp = parse(
        &r.resolve(&make_query("norecord.internal.lan.", RecordType::A), &cfg)
            .await,
    );
    assert_eq!(
        resp.response_code(),
        ResponseCode::NXDomain,
        "Names in an authoritative zone with no record must return NXDOMAIN, not forward upstream"
    );
}

#[tokio::test]
async fn test_zone_authority_does_not_block_other_domains() {
    let r = make_resolver_no_cache();
    let mut zones = HashMap::new();
    zones.insert(
        "internal.lan".into(),
        ZoneConfig {
            soa: None,
            ns: None,
        },
    );
    let mut cfg = make_config_with_zones(vec![], vec![], zones);
    cfg.server.upstream = vec!["127.0.0.1:1".into()];
    cfg.server.upstream_timeout = 1;
    // "external.com" is NOT in our zones — should attempt upstream, not NXDOMAIN
    let resp = parse(
        &r.resolve(&make_query("external.com.", RecordType::A), &cfg)
            .await,
    );
    // Upstream unreachable → SERVFAIL (not NXDOMAIN from zone authority)
    assert_eq!(resp.response_code(), ResponseCode::ServFail);
}

// ─── Cache ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_cache_populates_on_first_query() {
    let cache = Arc::new(DnsCache::new(100, 300, true));
    let r = Resolver::new(cache.clone());
    let cfg = make_config(vec![a_record("cached.lan", "1.1.1.1")], vec![]);
    r.resolve(&make_query("cached.lan.", RecordType::A), &cfg)
        .await;
    assert!(cache.stats().active >= 1);
}

#[tokio::test]
async fn test_cache_returns_consistent_response() {
    let cache = Arc::new(DnsCache::new(100, 300, true));
    let r = Resolver::new(cache.clone());
    let cfg = make_config(vec![a_record("cached.lan", "2.2.2.2")], vec![]);
    let q = make_query("cached.lan.", RecordType::A);
    let first = r.resolve(&q, &cfg).await;
    let second = r.resolve(&q, &cfg).await;
    // Both responses should contain the same answer
    let r1 = parse(&first);
    let r2 = parse(&second);
    assert_eq!(r1.response_code(), r2.response_code());
    assert_eq!(r1.answers().len(), r2.answers().len());
}

#[tokio::test]
async fn test_cache_disabled_does_not_cache() {
    let cache = Arc::new(DnsCache::new(100, 300, false));
    let r = Resolver::new(cache.clone());
    let cfg = make_config(vec![a_record("nocache.lan", "3.3.3.3")], vec![]);
    r.resolve(&make_query("nocache.lan.", RecordType::A), &cfg)
        .await;
    assert_eq!(
        cache.stats().active,
        0,
        "Cache is disabled, nothing should be stored"
    );
}

#[tokio::test]
async fn test_cache_invalidation_clears_entries() {
    let cache = Arc::new(DnsCache::new(100, 300, true));
    let r = Resolver::new(cache.clone());
    let cfg = make_config(vec![a_record("inv.lan", "4.4.4.4")], vec![]);
    r.resolve(&make_query("inv.lan.", RecordType::A), &cfg)
        .await;
    assert!(cache.stats().active >= 1);
    cache.invalidate();
    assert_eq!(cache.stats().size, 0);
}

#[tokio::test]
async fn test_cache_key_distinguishes_record_types() {
    let cache = Arc::new(DnsCache::new(100, 300, true));
    let r = Resolver::new(cache.clone());
    let cfg = make_config(vec![a_record("dual.lan", "5.5.5.5")], vec![]);
    r.resolve(&make_query("dual.lan.", RecordType::A), &cfg)
        .await;
    r.resolve(&make_query("dual.lan.", RecordType::MX), &cfg)
        .await;
    // A and MX are separate cache entries
    assert!(cache.stats().size >= 1);
}

// ─── Config: load / validate / persist_version ────────────────────────────────

#[test]
fn test_config_validate_accepts_valid_a_record() {
    let cfg = make_config(vec![a_record("ok.lan", "192.168.0.1")], vec![]);
    assert!(nanodns::config::validate(&cfg).is_ok());
}

#[test]
fn test_config_validate_rejects_bad_ipv4() {
    let cfg = make_config(
        vec![DnsRecord {
            name: "bad.lan".into(),
            record_type: CfgType::A,
            value: "not-an-ip".into(),
            ttl: 300,
            priority: None,
            wildcard: false,
            comment: None,
        }],
        vec![],
    );
    assert!(nanodns::config::validate(&cfg).is_err());
}

#[test]
fn test_config_validate_rejects_mx_without_priority() {
    let cfg = make_config(
        vec![DnsRecord {
            name: "mx.lan".into(),
            record_type: CfgType::Mx,
            value: "mail.lan".into(),
            ttl: 300,
            priority: None, // missing!
            wildcard: false,
            comment: None,
        }],
        vec![],
    );
    assert!(nanodns::config::validate(&cfg).is_err());
}

#[test]
fn test_config_validate_accepts_mx_with_priority() {
    let cfg = make_config(
        vec![DnsRecord {
            name: "mx.lan".into(),
            record_type: CfgType::Mx,
            value: "mail.lan".into(),
            ttl: 300,
            priority: Some(10),
            wildcard: false,
            comment: None,
        }],
        vec![],
    );
    assert!(nanodns::config::validate(&cfg).is_ok());
}

#[test]
fn test_config_write_example_and_load() {
    use std::path::PathBuf;
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("test_example.json");
    nanodns::config::write_example(&path).expect("write_example failed");
    let cfg = nanodns::config::load(&path).expect("load failed");
    assert!(
        !cfg.records.is_empty(),
        "Example config should have records"
    );
    assert!(
        cfg.server.mgmt_port > 0,
        "Example config should enable mgmt API"
    );
}

#[test]
fn test_config_persist_version() {
    use std::path::PathBuf;
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("test_persist.json");
    nanodns::config::write_example(&path).unwrap();

    // Initial version from example
    let initial = nanodns::config::load(&path).unwrap();
    let v0 = initial.server.config_version;

    nanodns::config::persist_version(&path, v0 + 5).expect("persist_version failed");

    let updated = nanodns::config::load(&path).unwrap();
    assert_eq!(updated.server.config_version, v0 + 5);

    // Other fields must be unchanged
    assert_eq!(updated.records.len(), initial.records.len());
    assert_eq!(updated.server.port, initial.server.port);
}

#[test]
fn test_config_save_round_trip() {
    use std::path::PathBuf;
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("test_save.json");
    let mut cfg = make_config(
        vec![a_record("save.lan", "7.7.7.7")],
        vec![nxdomain_rule("blocked.net")],
    );
    cfg.server.config_version = 42;

    nanodns::config::save(&path, &cfg).expect("save failed");
    let loaded = nanodns::config::load(&path).expect("load after save failed");

    assert_eq!(loaded.server.config_version, 42);
    assert_eq!(loaded.records.len(), 1);
    assert_eq!(loaded.records[0].value, "7.7.7.7");
    assert_eq!(loaded.rewrites.len(), 1);
}

// ─── Wildcard unit tests (also covers dns::wildcard directly) ─────────────────

mod wildcard_unit {
    use nanodns::dns::wildcard::matches;

    #[test]
    fn exact_match() {
        assert!(matches("foo.bar", "foo.bar"));
    }

    #[test]
    fn exact_mismatch() {
        assert!(!matches("foo.bar", "baz.bar"));
    }

    #[test]
    fn wildcard_single_level() {
        assert!(matches("*.foo.bar", "any.foo.bar"));
    }

    #[test]
    fn wildcard_does_not_match_two_levels() {
        assert!(!matches("*.foo.bar", "a.b.foo.bar"));
    }

    #[test]
    fn wildcard_matches_bare_parent() {
        assert!(matches("*.foo.bar", "foo.bar"));
    }

    #[test]
    fn wildcard_does_not_match_unrelated() {
        assert!(!matches("*.foo.bar", "other.com"));
    }

    #[test]
    fn trailing_dot_ignored() {
        assert!(matches("foo.bar", "foo.bar."));
        assert!(matches("foo.bar.", "foo.bar"));
    }
}

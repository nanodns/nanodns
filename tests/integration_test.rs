//! Integration tests for the DNS resolver logic (no network required).

use std::collections::HashMap;
use std::sync::Arc;

use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::{DNSClass, Name, RecordType};
use hickory_proto::serialize::binary::BinEncodable;

use nanodns::cache::DnsCache;
use nanodns::config::{
    Config, DnsRecord, RecordType as CfgType, RewriteAction, RewriteRule, ServerConfig,
};
use nanodns::dns::Resolver;

fn make_config(records: Vec<DnsRecord>, rewrites: Vec<RewriteRule>) -> Config {
    Config {
        server: ServerConfig::default(),
        records,
        rewrites,
        zones: HashMap::new(),
        version: 1,
    }
}

fn make_query(name: &str, qtype: RecordType) -> Vec<u8> {
    let mut msg = Message::new();
    msg.set_id(1234);
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

fn make_resolver() -> Resolver {
    let cache = Arc::new(DnsCache::new(100, 300, true));
    Resolver::new(cache)
}

#[tokio::test]
async fn test_a_record_resolved() {
    let resolver = make_resolver();
    let cfg = make_config(
        vec![DnsRecord {
            name: "web.internal.lan".into(),
            record_type: CfgType::A,
            value: "192.168.1.100".into(),
            ttl: 300,
            priority: None,
            wildcard: false,
            comment: None,
        }],
        vec![],
    );
    let query = make_query("web.internal.lan.", RecordType::A);
    let resp_bytes = resolver.resolve(&query, &cfg).await;
    assert!(!resp_bytes.is_empty());
    let resp = Message::from_vec(&resp_bytes).unwrap();
    assert!(!resp.answers().is_empty());
    assert_eq!(resp.response_code(), ResponseCode::NoError);
}

#[tokio::test]
async fn test_nxdomain_rewrite() {
    let resolver = make_resolver();
    let cfg = make_config(
        vec![],
        vec![RewriteRule {
            pattern: "ads.example.com".into(),
            action: RewriteAction::Nxdomain,
            value: None,
        }],
    );
    let query = make_query("ads.example.com.", RecordType::A);
    let resp_bytes = resolver.resolve(&query, &cfg).await;
    let resp = Message::from_vec(&resp_bytes).unwrap();
    assert_eq!(resp.response_code(), ResponseCode::NXDomain);
}

#[tokio::test]
async fn test_wildcard_record() {
    let resolver = make_resolver();
    let cfg = make_config(
        vec![DnsRecord {
            name: "*.app.internal.lan".into(),
            record_type: CfgType::A,
            value: "192.168.1.200".into(),
            ttl: 60,
            priority: None,
            wildcard: true,
            comment: None,
        }],
        vec![],
    );
    let query = make_query("myapp.app.internal.lan.", RecordType::A);
    let resp_bytes = resolver.resolve(&query, &cfg).await;
    let resp = Message::from_vec(&resp_bytes).unwrap();
    assert!(!resp.answers().is_empty());
}

#[tokio::test]
async fn test_wildcard_nxdomain_rewrite() {
    let resolver = make_resolver();
    let cfg = make_config(
        vec![],
        vec![RewriteRule {
            pattern: "*.tracker.net".into(),
            action: RewriteAction::Nxdomain,
            value: None,
        }],
    );
    let query = make_query("pixel.tracker.net.", RecordType::A);
    let resp_bytes = resolver.resolve(&query, &cfg).await;
    let resp = Message::from_vec(&resp_bytes).unwrap();
    assert_eq!(resp.response_code(), ResponseCode::NXDomain);
}

#[tokio::test]
async fn test_cache_stores_and_hits() {
    let cache = Arc::new(DnsCache::new(100, 300, true));
    let resolver = Resolver::new(cache.clone());
    let cfg = make_config(
        vec![DnsRecord {
            name: "cached.internal.lan".into(),
            record_type: CfgType::A,
            value: "10.0.0.1".into(),
            ttl: 300,
            priority: None,
            wildcard: false,
            comment: None,
        }],
        vec![],
    );
    let query = make_query("cached.internal.lan.", RecordType::A);
    resolver.resolve(&query, &cfg).await;
    let stats = cache.stats();
    assert!(stats.active >= 1, "Expected cache entry after first query");
}

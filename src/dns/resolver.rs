//! Core DNS resolver: local records → rewrites → upstream forwarding.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::RecordType;
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

use crate::cache::DnsCache;
use crate::config::{Config, RewriteAction};
use crate::dns::packet::{self, ensure_fqdn};
use crate::dns::wildcard;

pub struct Resolver {
    pub cache: Arc<DnsCache>,
}

impl Resolver {
    pub fn new(cache: Arc<DnsCache>) -> Self {
        Self { cache }
    }

    /// Resolve a raw DNS query packet; returns a raw DNS response packet.
    pub async fn resolve(&self, query_bytes: &[u8], config: &Config) -> Vec<u8> {
        let msg = match Message::from_vec(query_bytes) {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to parse DNS query: {}", e);
                return vec![];
            }
        };

        let response = self.handle(&msg, config).await;

        // Always log one line per query when log_queries is enabled
        if config.server.log_queries {
            if let Some(q) = msg.queries().first() {
                let name = q.name().to_string();
                let qtype = q.query_type();
                let rcode = response.response_code();
                let answers = response.answers().len();
                info!(
                    "Q {:<40} {:?}  → {} (answers={})",
                    name.trim_end_matches('.'),
                    qtype,
                    rcode,
                    answers
                );
            }
        }

        match response.to_bytes() {
            Ok(b) => b,
            Err(e) => {
                warn!("Failed to encode DNS response: {}", e);
                packet::servfail(&msg).to_bytes().unwrap_or_default()
            }
        }
    }

    async fn handle(&self, query: &Message, config: &Config) -> Message {
        let q = match query.queries().first() {
            Some(q) => q,
            None => return packet::servfail(query),
        };

        let name = q.name().to_lowercase().to_string();
        let qtype = q.query_type();
        let name_bare = name.trim_end_matches('.');

        if config.server.log_queries {
            info!("query {} {:?}", name_bare, qtype);
        }

        // ── Cache lookup ──────────────────────────────────────────────────────
        let cache_key = DnsCache::key(name_bare, qtype.into());
        if let Some(cached) = self.cache.get(&cache_key) {
            debug!("cache hit {}", cache_key);
            if let Ok(mut m) = Message::from_vec(&cached) {
                m.set_id(query.id());
                return m;
            }
        }

        // ── Rewrite rules ─────────────────────────────────────────────────────
        for rule in &config.rewrites {
            if wildcard::matches(&rule.pattern, name_bare) {
                match rule.action {
                    RewriteAction::Nxdomain => {
                        debug!("Rewrite NXDOMAIN: {}", name_bare);
                        return packet::nxdomain(query);
                    }
                    RewriteAction::Redirect => {
                        // Redirect: return A record with rule value if provided
                        if let Some(ip_str) = &rule.value {
                            if let Ok(ip) = ip_str.parse::<std::net::Ipv4Addr>() {
                                return self.build_a_response(query, &name, ip, 60);
                            }
                        }
                    }
                }
            }
        }

        // ── Local records ─────────────────────────────────────────────────────
        let local = self.resolve_local(query, name_bare, qtype, config);
        if let Some(resp) = local {
            if config.server.log_queries {
                info!(
                    "local  {} {:?} → {} answers",
                    name_bare, qtype,
                    resp.answers().len()
                );
            }
            // Cache it
            if let Ok(bytes) = resp.to_bytes() {
                let ttl = resp.answers().first().map(|r| r.ttl());
                self.cache.set(cache_key, bytes.clone(), ttl);
            }
            return resp;
        }

        // ── Upstream forwarding ───────────────────────────────────────────────
        if config.server.log_queries {
            info!("forward {} {:?} → upstream", name_bare, qtype);
        }
        match self.forward(query, &config.server.upstream).await {
            Ok(resp) => {
                if config.server.log_queries {
                    info!(
                        "upstream {} {:?} → rcode={:?} answers={}",
                        name_bare, qtype,
                        resp.response_code(),
                        resp.answers().len()
                    );
                }
                if let Ok(bytes) = resp.to_bytes() {
                    let ttl = resp.answers().first().map(|r| r.ttl());
                    self.cache.set(cache_key, bytes, ttl);
                }
                resp
            }
            Err(e) => {
                warn!("Upstream error for {}: {}", name_bare, e);
                packet::servfail(query)
            }
        }
    }

    fn resolve_local(
        &self,
        query: &Message,
        name: &str,
        qtype: RecordType,
        config: &Config,
    ) -> Option<Message> {
        let mut answers = Vec::new();

        for record in &config.records {
            let rec_name = record.name.trim_end_matches('.');

            // Match: exact or wildcard
            let name_matches = if record.wildcard {
                wildcard::matches(rec_name, name)
            } else {
                rec_name.eq_ignore_ascii_case(name)
            };

            if !name_matches { continue; }

            let rec_qtype = packet::map_qtype(&record.record_type);

            // Type match or CNAME (always include CNAME for the name)
            if rec_qtype == qtype || rec_qtype == RecordType::CNAME {
                if let Some(rr) = packet::to_rr(record) {
                    answers.push(rr);
                }
            }
        }

        if answers.is_empty() {
            return None;
        }

        let mut resp = Message::new();
        resp.set_id(query.id());
        resp.set_message_type(MessageType::Response);
        resp.set_op_code(OpCode::Query);
        resp.set_authoritative(true);
        resp.set_response_code(ResponseCode::NoError);
        if let Some(q) = query.queries().first() {
            resp.add_query(q.clone());
        }
        for a in answers {
            resp.add_answer(a);
        }

        Some(resp)
    }

    fn build_a_response(
        &self,
        query: &Message,
        name: &str,
        ip: std::net::Ipv4Addr,
        ttl: u32,
    ) -> Message {
        use hickory_proto::rr::{Name, RData, Record};
        use hickory_proto::rr::rdata::A;
        use std::str::FromStr;

        let mut resp = Message::new();
        resp.set_id(query.id());
        resp.set_message_type(MessageType::Response);
        resp.set_op_code(OpCode::Query);
        resp.set_response_code(ResponseCode::NoError);
        if let Some(q) = query.queries().first() {
            resp.add_query(q.clone());
        }
        if let Ok(n) = Name::from_str(&ensure_fqdn(name)) {
            let mut rec = Record::new();
            rec.set_name(n).set_ttl(ttl).set_rr_type(RecordType::A).set_data(Some(RData::A(A(ip))));
            resp.add_answer(rec);
        }
        resp
    }

    async fn forward(
        &self,
        query: &Message,
        upstream_servers: &[String],
    ) -> anyhow::Result<Message> {
        let query_bytes = query.to_bytes()?;

        for server in upstream_servers {
            let addr = if server.contains(':') {
                server.parse::<SocketAddr>()?
            } else {
                format!("{}:53", server).parse::<SocketAddr>()?
            };

            match self.send_udp(&query_bytes, addr).await {
                Ok(resp_bytes) => {
                    let resp = Message::from_vec(&resp_bytes)?;
                    return Ok(resp);
                }
                Err(e) => {
                    warn!("Upstream {} failed: {}", server, e);
                    continue;
                }
            }
        }

        anyhow::bail!("All upstream servers failed");
    }

    async fn send_udp(&self, query: &[u8], addr: SocketAddr) -> anyhow::Result<Vec<u8>> {
        // Use unconnected send_to/recv_from to avoid Windows WSAECONNRESET (10054)
        // that occurs with connect() when a previous packet gets an ICMP rejection.
        let sock = UdpSocket::bind("0.0.0.0:0").await?;
        sock.send_to(query, addr).await?;

        let mut buf = vec![0u8; 4096];
        let (n, _) = tokio::time::timeout(
            Duration::from_secs(3),
            sock.recv_from(&mut buf),
        )
        .await??;
        Ok(buf[..n].to_vec())
    }
}

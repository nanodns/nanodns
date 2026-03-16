use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_upstream")]
    pub upstream: Vec<String>,
    #[serde(default = "bool_true")]
    pub cache_enabled: bool,
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl: u32,
    #[serde(default = "default_cache_size")]
    pub cache_size: usize,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub log_queries: bool,
    #[serde(default = "bool_true")]
    pub hot_reload: bool,
    pub mgmt_port: Option<u16>,
    #[serde(default)]
    pub mgmt_host: Option<String>,
    #[serde(default)]
    pub peers: Vec<String>,
}

fn default_host() -> String { "0.0.0.0".into() }
fn default_port() -> u16 { 53 }
fn default_upstream() -> Vec<String> { vec!["8.8.8.8".into(), "1.1.1.1".into()] }
fn bool_true() -> bool { true }
fn default_cache_ttl() -> u32 { 300 }
fn default_cache_size() -> usize { 1000 }
fn default_log_level() -> String { "INFO".into() }

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            upstream: default_upstream(),
            cache_enabled: true,
            cache_ttl: default_cache_ttl(),
            cache_size: default_cache_size(),
            log_level: default_log_level(),
            log_queries: false,
            hot_reload: true,
            mgmt_port: Some(9053),
            mgmt_host: None,
            peers: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "UPPERCASE")]
pub enum RecordType {
    A,
    #[serde(rename = "AAAA")]
    Aaaa,
    #[serde(rename = "CNAME")]
    Cname,
    #[serde(rename = "MX")]
    Mx,
    #[serde(rename = "TXT")]
    Txt,
    #[serde(rename = "PTR")]
    Ptr,
    #[serde(rename = "NS")]
    Ns,
    #[serde(rename = "SOA")]
    Soa,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsRecord {
    pub name: String,
    #[serde(rename = "type")]
    pub record_type: RecordType,
    pub value: String,
    #[serde(default = "default_ttl")]
    pub ttl: u32,
    #[serde(default)]
    pub priority: Option<u16>,
    #[serde(default)]
    pub wildcard: bool,
    #[serde(default)]
    pub comment: Option<String>,
}

fn default_ttl() -> u32 { 300 }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RewriteAction {
    Nxdomain,
    Redirect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteRule {
    #[serde(rename = "match")]
    pub pattern: String,
    pub action: RewriteAction,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoaRecord {
    pub mname: String,
    pub rname: String,
    pub serial: u32,
    pub refresh: u32,
    pub retry: u32,
    pub expire: u32,
    pub minimum: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneConfig {
    pub soa: Option<SoaRecord>,
    pub ns: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub records: Vec<DnsRecord>,
    #[serde(default)]
    pub rewrites: Vec<RewriteRule>,
    #[serde(default)]
    pub zones: HashMap<String, ZoneConfig>,
    #[serde(default)]
    pub version: u64,
}

pub fn load(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read config: {}", path.display()))?;
    let cfg: Config = serde_json::from_str(&content)
        .with_context(|| format!("Cannot parse config: {}", path.display()))?;
    validate(&cfg)?;
    Ok(cfg)
}

pub fn validate(cfg: &Config) -> Result<()> {
    for r in &cfg.records {
        if r.record_type == RecordType::A {
            r.value.parse::<std::net::Ipv4Addr>()
                .with_context(|| format!("Record '{}': invalid IPv4 '{}'", r.name, r.value))?;
        }
        if r.record_type == RecordType::Aaaa {
            r.value.parse::<std::net::Ipv6Addr>()
                .with_context(|| format!("Record '{}': invalid IPv6 '{}'", r.name, r.value))?;
        }
        if r.record_type == RecordType::Mx && r.priority.is_none() {
            anyhow::bail!("MX record '{}' requires 'priority' field", r.name);
        }
    }
    Ok(())
}

pub fn write_example(path: &Path) -> Result<()> {
    let example = serde_json::json!({
        "server": {
            "host": "0.0.0.0",
            "port": 53,
            "upstream": ["8.8.8.8", "1.1.1.1"],
            "cache_enabled": true,
            "cache_ttl": 300,
            "cache_size": 1000,
            "log_level": "INFO",
            "log_queries": false,
            "hot_reload": true,
            "mgmt_port": 9053,
            "peers": []
        },
        "zones": {
            "internal.lan": {
                "soa": {
                    "mname": "ns1.internal.lan",
                    "rname": "admin.internal.lan",
                    "serial": 2024010101u64,
                    "refresh": 3600,
                    "retry": 900,
                    "expire": 604800,
                    "minimum": 300
                },
                "ns": ["ns1.internal.lan"]
            }
        },
        "records": [
            { "name": "web.internal.lan",   "type": "A",     "value": "192.168.1.100", "ttl": 300 },
            { "name": "db.internal.lan",    "type": "A",     "value": "192.168.1.101" },
            { "name": "api.internal.lan",   "type": "CNAME", "value": "web.internal.lan" },
            { "name": "internal.lan",       "type": "MX",    "value": "mail.internal.lan", "priority": 10 },
            { "name": "*.app.internal.lan", "type": "A",     "value": "192.168.1.200", "wildcard": true },
            { "name": "internal.lan",       "type": "TXT",   "value": "v=spf1 mx ~all" }
        ],
        "rewrites": [
            { "match": "ads.example.com", "action": "nxdomain" },
            { "match": "*.tracker.net",   "action": "nxdomain" }
        ]
    });
    let json = serde_json::to_string_pretty(&example)?;
    std::fs::write(path, json)?;
    Ok(())
}

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

    /// Seconds before a single upstream attempt is abandoned
    #[serde(default = "default_upstream_timeout")]
    pub upstream_timeout: u64,

    /// Port used when contacting upstream resolvers
    #[serde(default = "default_upstream_port")]
    pub upstream_port: u16,

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

    /// Management API listen address
    #[serde(default = "default_mgmt_host")]
    pub mgmt_host: String,

    /// Management API port — 0 = disabled
    #[serde(default)] // default is 0 (disabled)
    pub mgmt_port: u16,

    /// Peer management addresses in "host:port" format
    #[serde(default)]
    pub peers: Vec<String>,

    /// Monotonic version counter — managed automatically, never edit by hand
    #[serde(default = "default_version")]
    pub config_version: u64,
}

fn default_host() -> String {
    "0.0.0.0".into()
}
fn default_port() -> u16 {
    53
}
fn default_upstream() -> Vec<String> {
    vec!["8.8.8.8".into(), "1.1.1.1".into()]
}
fn default_upstream_timeout() -> u64 {
    3
}
fn default_upstream_port() -> u16 {
    53
}
fn bool_true() -> bool {
    true
}
fn default_cache_ttl() -> u32 {
    300
}
fn default_cache_size() -> usize {
    1000
}
fn default_log_level() -> String {
    "INFO".into()
}
fn default_mgmt_host() -> String {
    "0.0.0.0".into()
}
fn default_version() -> u64 {
    1
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            upstream: default_upstream(),
            upstream_timeout: default_upstream_timeout(),
            upstream_port: default_upstream_port(),
            cache_enabled: true,
            cache_ttl: default_cache_ttl(),
            cache_size: default_cache_size(),
            log_level: default_log_level(),
            log_queries: false,
            hot_reload: true,
            mgmt_host: default_mgmt_host(),
            mgmt_port: 0, // disabled by default
            peers: vec![],
            config_version: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum RecordType {
    #[serde(rename = "A")]
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
    /// Match only direct (single-level) subdomains
    #[serde(default)]
    pub wildcard: bool,
    #[serde(default)]
    pub comment: Option<String>,
}

fn default_ttl() -> u32 {
    300
}

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
    #[serde(default)]
    pub comment: Option<String>,
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
}

pub fn load(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read config: {}", path.display()))?;
    let cfg: Config = serde_json::from_str(&content)
        .with_context(|| format!("Cannot parse config: {}", path.display()))?;
    validate(&cfg)?;
    Ok(cfg)
}

/// Write the full `Config` back to disk (used after peer-sync so all fields
/// including records, rewrites, zones are persisted, not just the version).
pub fn save(path: &Path, cfg: &Config) -> Result<()> {
    let json = serde_json::to_string_pretty(cfg).context("Cannot serialize config")?;
    std::fs::write(path, json)
        .with_context(|| format!("Cannot write config to {}", path.display()))?;
    Ok(())
}

/// Persist only `server.config_version` back into the existing config file
/// using a read-modify-write so all other fields and formatting are preserved.
/// Used after hot-reload (the file content is already correct, only version changes).
pub fn persist_version(path: &Path, version: u64) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read config for version persist: {}", path.display()))?;

    let mut value: serde_json::Value =
        serde_json::from_str(&content).context("Cannot parse config JSON for version persist")?;

    if let Some(server) = value.get_mut("server").and_then(|s| s.as_object_mut()) {
        server.insert("config_version".to_string(), serde_json::json!(version));
    } else {
        value["server"] = serde_json::json!({ "config_version": version });
    }

    let updated = serde_json::to_string_pretty(&value)?;
    std::fs::write(path, updated)
        .with_context(|| format!("Cannot write config version to {}", path.display()))?;

    Ok(())
}

pub fn validate(cfg: &Config) -> Result<()> {
    for r in &cfg.records {
        if r.record_type == RecordType::A {
            r.value
                .parse::<std::net::Ipv4Addr>()
                .with_context(|| format!("Record '{}': invalid IPv4 '{}'", r.name, r.value))?;
        }
        if r.record_type == RecordType::Aaaa {
            r.value
                .parse::<std::net::Ipv6Addr>()
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
            "upstream_timeout": 3,
            "upstream_port": 53,
            "cache_enabled": true,
            "cache_ttl": 300,
            "cache_size": 1000,
            "log_level": "INFO",
            "log_queries": false,
            "hot_reload": true,
            "mgmt_host": "0.0.0.0",
            "mgmt_port": 9053,
            "peers": [],
            "config_version": 1
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
            { "name": "app.internal.lan",   "type": "A",     "value": "192.168.1.200", "wildcard": true,
              "comment": "matches foo.app.internal.lan but NOT a.b.app.internal.lan" },
            { "name": "internal.lan",       "type": "TXT",   "value": "v=spf1 mx ~all" }
        ],
        "rewrites": [
            { "match": "ads.example.com",  "action": "nxdomain", "comment": "block ads" },
            { "match": "*.tracker.net",    "action": "nxdomain" }
        ]
    });
    let json = serde_json::to_string_pretty(&example)?;
    std::fs::write(path, json)?;
    Ok(())
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn test_default_server_config() {
        let s = ServerConfig::default();
        assert_eq!(s.port, 53);
        assert_eq!(s.host, "0.0.0.0");
        assert!(s.cache_enabled);
        assert!(s.hot_reload);
        assert_eq!(s.mgmt_port, 0); // disabled by default
        assert_eq!(s.upstream_timeout, 3);
        assert_eq!(s.upstream_port, 53);
        assert_eq!(s.config_version, 1);
    }

    #[test]
    fn test_validate_valid_a_record() {
        let cfg = Config {
            server: ServerConfig::default(),
            records: vec![DnsRecord {
                name: "ok.lan".into(),
                record_type: RecordType::A,
                value: "1.2.3.4".into(),
                ttl: 60,
                priority: None,
                wildcard: false,
                comment: None,
            }],
            rewrites: vec![],
            zones: std::collections::HashMap::new(),
        };
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn test_validate_invalid_ipv4() {
        let cfg = Config {
            server: ServerConfig::default(),
            records: vec![DnsRecord {
                name: "bad.lan".into(),
                record_type: RecordType::A,
                value: "999.999.999.999".into(),
                ttl: 300,
                priority: None,
                wildcard: false,
                comment: None,
            }],
            rewrites: vec![],
            zones: std::collections::HashMap::new(),
        };
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn test_validate_invalid_ipv6() {
        let cfg = Config {
            server: ServerConfig::default(),
            records: vec![DnsRecord {
                name: "bad.lan".into(),
                record_type: RecordType::Aaaa,
                value: "not:valid:ipv6".into(),
                ttl: 300,
                priority: None,
                wildcard: false,
                comment: None,
            }],
            rewrites: vec![],
            zones: std::collections::HashMap::new(),
        };
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn test_validate_mx_requires_priority() {
        let cfg = Config {
            server: ServerConfig::default(),
            records: vec![DnsRecord {
                name: "mail.lan".into(),
                record_type: RecordType::Mx,
                value: "mx.lan".into(),
                ttl: 300,
                priority: None,
                wildcard: false,
                comment: None,
            }],
            rewrites: vec![],
            zones: std::collections::HashMap::new(),
        };
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn test_load_nonexistent_returns_error() {
        assert!(load(Path::new("/nonexistent/path/nanodns.json")).is_err());
    }

    #[test]
    fn test_write_example_creates_valid_file() {
        let path = tmp("test_cfg_example.json");
        write_example(&path).unwrap();
        let cfg = load(&path).unwrap();
        assert!(!cfg.records.is_empty());
        assert_eq!(cfg.server.port, 53);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_persist_version_updates_only_version() {
        let path = tmp("test_persist_v.json");
        write_example(&path).unwrap();
        let original = load(&path).unwrap();
        let original_records = original.records.len();

        persist_version(&path, 99).unwrap();
        let updated = load(&path).unwrap();

        assert_eq!(updated.server.config_version, 99);
        assert_eq!(
            updated.records.len(),
            original_records,
            "persist_version must not change records"
        );
        assert_eq!(
            updated.server.port, original.server.port,
            "persist_version must not change other server fields"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_save_round_trip() {
        let path = tmp("test_save_rt.json");
        let cfg = Config {
            server: ServerConfig {
                config_version: 77,
                port: 5353,
                ..ServerConfig::default()
            },
            records: vec![DnsRecord {
                name: "rt.lan".into(),
                record_type: RecordType::A,
                value: "10.20.30.40".into(),
                ttl: 120,
                priority: None,
                wildcard: false,
                comment: Some("round trip".into()),
            }],
            rewrites: vec![],
            zones: std::collections::HashMap::new(),
        };

        save(&path, &cfg).unwrap();
        let loaded = load(&path).unwrap();

        assert_eq!(loaded.server.config_version, 77);
        assert_eq!(loaded.server.port, 5353);
        assert_eq!(loaded.records[0].value, "10.20.30.40");
        assert_eq!(loaded.records[0].ttl, 120);
        std::fs::remove_file(&path).ok();
    }
}

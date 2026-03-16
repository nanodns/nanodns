//! DNS response cache with TTL expiry

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct CacheEntry {
    data: Vec<u8>,
    expires_at: Instant,
}

pub struct DnsCache {
    entries: Mutex<HashMap<String, CacheEntry>>,
    max_size: usize,
    default_ttl: Duration,
    enabled: bool,
}

impl DnsCache {
    pub fn new(max_size: usize, default_ttl: u32, enabled: bool) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            max_size,
            default_ttl: Duration::from_secs(default_ttl as u64),
            enabled,
        }
    }

    /// Build a cache key from query name + type
    pub fn key(name: &str, qtype: u16) -> String {
        format!("{}:{}", name.to_lowercase(), qtype)
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        if !self.enabled {
            return None;
        }
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.get(key) {
            if entry.expires_at > Instant::now() {
                return Some(entry.data.clone());
            }
            entries.remove(key);
        }
        None
    }

    pub fn set(&self, key: String, data: Vec<u8>, ttl: Option<u32>) {
        if !self.enabled {
            return;
        }
        let ttl = ttl
            .map(|t| Duration::from_secs(t as u64))
            .unwrap_or(self.default_ttl);
        let entry = CacheEntry {
            data,
            expires_at: Instant::now() + ttl,
        };
        let mut entries = self.entries.lock().unwrap();
        // Simple eviction: if at capacity, clear expired first
        if entries.len() >= self.max_size {
            let now = Instant::now();
            entries.retain(|_, v| v.expires_at > now);
        }
        if entries.len() < self.max_size {
            entries.insert(key, entry);
        }
    }

    pub fn invalidate(&self) {
        let mut entries = self.entries.lock().unwrap();
        entries.clear();
    }

    pub fn stats(&self) -> CacheStats {
        let entries = self.entries.lock().unwrap();
        let now = Instant::now();
        let active = entries.values().filter(|e| e.expires_at > now).count();
        CacheStats {
            size: entries.len(),
            active,
            capacity: self.max_size,
        }
    }
}

pub struct CacheStats {
    pub size: usize,
    pub active: usize,
    pub capacity: usize,
}

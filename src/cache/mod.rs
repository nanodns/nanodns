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

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get() {
        let c = DnsCache::new(10, 300, true);
        let key = DnsCache::key("foo.lan", 1);
        c.set(key.clone(), vec![1, 2, 3], None);
        assert_eq!(c.get(&key), Some(vec![1, 2, 3]));
    }

    #[test]
    fn test_get_miss_returns_none() {
        let c = DnsCache::new(10, 300, true);
        assert_eq!(c.get("nonexistent"), None);
    }

    #[test]
    fn test_disabled_cache_never_stores() {
        let c = DnsCache::new(10, 300, false);
        let key = DnsCache::key("foo.lan", 1);
        c.set(key.clone(), vec![1], None);
        assert_eq!(c.get(&key), None);
        assert_eq!(c.stats().size, 0);
    }

    #[test]
    fn test_invalidate_clears_all() {
        let c = DnsCache::new(10, 300, true);
        c.set(DnsCache::key("a.lan", 1), vec![1], None);
        c.set(DnsCache::key("b.lan", 1), vec![2], None);
        assert!(c.stats().size > 0);
        c.invalidate();
        assert_eq!(c.stats().size, 0);
    }

    #[test]
    fn test_custom_ttl_zero_expires_immediately() {
        let c = DnsCache::new(10, 300, true);
        let key = DnsCache::key("ttl.lan", 1);
        c.set(key.clone(), vec![9], Some(0)); // TTL = 0 s
                                              // Entry may or may not be stored depending on timing, but it should
                                              // not appear as active
        let stats = c.stats();
        assert_eq!(stats.active, 0);
    }

    #[test]
    fn test_key_includes_qtype() {
        let k1 = DnsCache::key("foo.lan", 1);
        let k2 = DnsCache::key("foo.lan", 28);
        assert_ne!(k1, k2, "A and AAAA must have different cache keys");
    }

    #[test]
    fn test_stats_reports_capacity() {
        let c = DnsCache::new(42, 300, true);
        assert_eq!(c.stats().capacity, 42);
    }

    #[test]
    fn test_eviction_at_capacity() {
        let c = DnsCache::new(2, 300, true);
        c.set(DnsCache::key("a.lan", 1), vec![1], None);
        c.set(DnsCache::key("b.lan", 1), vec![2], None);
        // Third entry should evict one of the expired/oldest entries
        c.set(DnsCache::key("c.lan", 1), vec![3], None);
        assert!(c.stats().size <= 2);
    }
}

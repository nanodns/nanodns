# NanoDNS (Rust)

A lightweight, high-performance DNS server for internal networks — configured with a single JSON file. Rust rewrite of [iyuangang/nanodns](https://github.com/iyuangang/nanodns).

## Why Rust?

| | Python NanoDNS | Rust NanoDNS |
|---|---|---|
| Binary size | ~300 MB (with interpreter) | ~3 MB (statically linked) |
| Memory at idle | ~30 MB | ~2 MB |
| Startup time | ~1 s | <10 ms |
| Container base | Python distroless | `scratch` (zero OS) |
| DNS throughput | ~10 k qps | ~200 k+ qps |

## Quick start

```bash
# Build
cargo build --release

# Write an example config
./target/release/nanodns init

# Validate it
./target/release/nanodns check nanodns.json

# Run on port 5353 (no root needed)
./target/release/nanodns start --port 5353

# Query it
dig @127.0.0.1 -p 5353 web.internal.lan A
```

Port 53 (requires root or `CAP_NET_BIND_SERVICE`):
```bash
sudo ./target/release/nanodns start --config /etc/nanodns/nanodns.json
```

## Configuration

Identical JSON format to the original Python project:

```json
{
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
  "records": [
    { "name": "web.internal.lan",   "type": "A",     "value": "192.168.1.100", "ttl": 300 },
    { "name": "api.internal.lan",   "type": "CNAME", "value": "web.internal.lan" },
    { "name": "internal.lan",       "type": "MX",    "value": "mail.internal.lan", "priority": 10 },
    { "name": "*.app.internal.lan", "type": "A",     "value": "192.168.1.200", "wildcard": true }
  ],
  "rewrites": [
    { "match": "ads.example.com", "action": "nxdomain" },
    { "match": "*.tracker.net",   "action": "nxdomain" }
  ]
}
```

Hot-reload is active by default — any file change is picked up within 5 seconds with no restart.

## Record types

| Type | `value` | Extra |
|------|---------|-------|
| `A` | IPv4 address | — |
| `AAAA` | IPv6 address | — |
| `CNAME` | Target hostname | — |
| `MX` | Mail hostname | `priority` (int) |
| `TXT` | Text string | — |
| `PTR` | Pointer hostname | — |
| `NS` | Nameserver hostname | — |

## Management API

Enable with `"mgmt_port": 9053` in config.

| Endpoint | Method | Description |
|---|---|---|
| `/health` | GET | Liveness |
| `/ready` | GET | Readiness |
| `/metrics` | GET | Cache stats, query count, uptime, version |
| `/cluster` | GET | All peers with version + reachability |
| `/config/raw` | GET | Full config JSON (used by peer catch-up) |
| `/reload` | POST | Reload from disk, bump version, push to peers |
| `/sync` | POST | Accept versioned config push from a peer |

```bash
# Trigger reload
curl -X POST http://localhost:9053/reload

# Check cluster status
curl -s http://localhost:9053/cluster | python3 -m json.tool

# Metrics
curl -s http://localhost:9053/metrics
```

## Multi-node HA

```json
{
  "server": {
    "mgmt_port": 9053,
    "peers": ["10.0.0.12:9053", "10.0.0.13:9053"]
  }
}
```

Sync behaviour matches the original Python version:
- **Push**: on `/reload`, config is immediately pushed to all online peers (<1 s).
- **Pull**: a background reconcile loop every 30 s catches up offline nodes.
- **Version wins**: highest version always prevails, no split-brain.

## Docker

```bash
# Single node
docker compose up -d

# Query through container
dig @127.0.0.1 web.internal.lan A
```

## CLI

```
nanodns start   --config FILE [--host HOST] [--port PORT] [--log-level LEVEL] [--no-cache]
nanodns init    [OUTPUT]       Write an example config
nanodns check   CONFIG         Validate config and print summary
nanodns --version
```

## Project layout

```
src/
├── main.rs          CLI entry (clap): start / init / check
├── error.rs         Unified error types (thiserror)
├── config/
│   └── mod.rs       Config structs, load(), validate(), write_example()
├── dns/
│   ├── mod.rs
│   ├── resolver.rs  Core: local records → rewrites → upstream UDP forward
│   ├── packet.rs    hickory-proto helpers: build DNS response messages
│   └── wildcard.rs  Wildcard pattern matching (*.foo.bar)
├── cache/
│   └── mod.rs       TTL-aware response cache (Mutex<HashMap>)
├── server/
│   └── mod.rs       tokio UDP loop, hot-reload watcher (ArcSwap state)
├── mgmt/
│   └── mod.rs       axum HTTP API: /health /metrics /reload /cluster /sync
└── sync/
    └── mod.rs       Peer version probing + reconcile loop (reqwest)
```

## License

MIT

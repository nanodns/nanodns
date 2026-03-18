<div align="center">

# NanoDNS

**A zero-dependency DNS server for internal networks.**  
One JSON file. A single 3 MB binary. Runs anywhere.

[![CI](https://github.com/nanodns/nanodns/actions/workflows/ci.yml/badge.svg)](https://github.com/nanodns/nanodns/actions/workflows/test.yml)
[![Release](https://github.com/nanodns/nanodns/actions/workflows/release.yml/badge.svg)](https://github.com/nanodns/nanodns/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![GHCR](https://img.shields.io/badge/image-ghcr.io-blue?logo=github)](https://github.com/nanodns/nanodns/pkgs/container/nanodns)

</div>

---

You need internal DNS for your homelab, a small team, or a dev environment.  
You don't need a 300 MB container, a web UI, a PostgreSQL backend, or a BIND config that requires a PhD to edit.

```bash
nanodns init             # writes nanodns.json in the current directory
nanodns start            # listening on :53 in under 10 ms
```

```json
{ "records": [
    { "name": "dev.local",   "type": "A", "value": "192.168.1.10" },
    { "name": "*.dev.local", "type": "A", "value": "192.168.1.10", "wildcard": true }
]}
```

```bash
$ dig @127.0.0.1 api.dev.local +short
192.168.1.10    # wildcard matched — no restart needed
```

---

## Why NanoDNS (Rust)?

The original [Python NanoDNS](https://github.com/nanodns/nanodns) is great. We kept everything that made it useful and rewrote the internals in Rust for deployments where resource consumption and reliability matter.

|  | Python NanoDNS | **NanoDNS (Rust)** |
|--|:-:|:-:|
| Binary / image size | ~300 MB | **~3 MB** |
| Memory at idle | ~30 MB | **~2 MB** |
| Startup time | ~1 s | **< 10 ms** |
| DNS throughput | ~10 k qps | **~200 k+ qps** |
| Container base | Python distroless | **Chainguard distroless** |
| Config format | JSON | **Same JSON — 100% compatible** |
| Hot-reload | ✅ | ✅ |
| Multi-node HA | ✅ | ✅ |
| Memory safety | — | **Guaranteed by the compiler** |

> **100% config-compatible.** If you already run Python NanoDNS, drop in this binary and your `nanodns.json` works as-is.

---

## Quickstart — 60 seconds

### Download a pre-built binary

Every release ships statically-linked binaries for six platforms. No runtime, no installer.

```bash
# Linux x86_64
curl -Lo nanodns.tar.gz \
  https://github.com/nanodns/nanodns/releases/latest/download/nanodns-linux-x86_64.tar.gz
tar xzf nanodns.tar.gz && chmod +x nanodns

# Linux ARM64 (Raspberry Pi 4, ARM servers)
curl -Lo nanodns.tar.gz \
  https://github.com/nanodns/nanodns/releases/latest/download/nanodns-linux-aarch64.tar.gz
tar xzf nanodns.tar.gz && chmod +x nanodns

# macOS Apple Silicon
curl -Lo nanodns.tar.gz \
  https://github.com/nanodns/nanodns/releases/latest/download/nanodns-macos-aarch64.tar.gz
tar xzf nanodns.tar.gz && chmod +x nanodns
```

Verify the SHA-256 checksum against `CHECKSUMS.txt` on the [releases page](https://github.com/nanodns/nanodns/releases) before running.

### Build from source

```bash
git clone https://github.com/nanodns/nanodns
cd nanodns
cargo build --release    # requires Rust >= 1.77
```

### Run

```bash
nanodns init                    # generates nanodns.json
nanodns check nanodns.json      # validates before starting
nanodns start --port 5353       # non-privileged port for testing

# Port 53 requires root or CAP_NET_BIND_SERVICE
sudo nanodns start
```

```bash
$ dig @127.0.0.1 -p 5353 web.internal.lan A +short
192.168.1.100
```

---

## Configuration

A single JSON file controls everything. Edit it while running — changes are detected within 5 seconds with zero downtime.

```json
{
  "server": {
    "host":             "0.0.0.0",
    "port":             53,
    "upstream":         ["8.8.8.8", "1.1.1.1"],
    "upstream_timeout": 3,
    "cache_enabled":    true,
    "cache_ttl":        300,
    "cache_size":       1000,
    "log_level":        "INFO",
    "log_queries":      true,
    "hot_reload":       true,
    "mgmt_host":        "0.0.0.0",
    "mgmt_port":        9053,
    "peers":            []
  },
  "zones": {
    "internal.lan": {
      "soa": {
        "mname": "ns1.internal.lan", "rname": "admin.internal.lan",
        "serial": 2024010101, "refresh": 3600, "retry": 900,
        "expire": 604800, "minimum": 300
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
    { "match": "ads.example.com", "action": "nxdomain", "comment": "block ads" },
    { "match": "*.tracker.net",   "action": "nxdomain" }
  ]
}
```

### Record types

| Type | `value` | Notes |
|------|---------|-------|
| `A` | IPv4 address | Multiple A records → automatic round-robin |
| `AAAA` | IPv6 address | |
| `CNAME` | Target hostname | |
| `MX` | Mail hostname | Requires `priority` — lower = higher preference |
| `TXT` | Text string | SPF, DKIM, verification tokens |
| `PTR` | Pointer hostname | Reverse DNS |
| `NS` | Nameserver hostname | |

All records accept: `ttl` (seconds, default `300`), `wildcard` (bool), `comment` (ignored at runtime).

### Wildcard records

```json
{ "name": "app.internal.lan", "type": "A", "value": "192.168.1.200", "wildcard": true }
```

Matches `foo.app.internal.lan` and `bar.app.internal.lan` — but **not** `a.b.app.internal.lan` (single level only).

### Domain blocking

```json
{ "match": "doubleclick.net",   "action": "nxdomain" },
{ "match": "*.doubleclick.net", "action": "nxdomain" }
```

Blocked names return `NXDOMAIN` in sub-millisecond time — no upstream query, no cache write.

### Zone authority

Names inside a declared `zone` that have no matching record return `NXDOMAIN` immediately and are **never** forwarded upstream. This lets you own an entire private domain cleanly without leaking queries to the internet.

---

## Hot Reload

When `hot_reload: true`, NanoDNS polls the config file every 5 seconds.

On a valid change:

1. New config is parsed and validated — bad JSON or invalid records are rejected; the current config keeps serving with zero downtime.
2. Records are swapped atomically and the cache is flushed.
3. In HA mode, the new config is pushed to all peers immediately.

```bash
# Trigger an immediate reload without waiting 5 s
curl -X POST http://localhost:9053/reload
```

---

## Multi-node HA

No Zookeeper. No Raft. No etcd. Just point each node at its peers.

```json
{
  "server": {
    "mgmt_port": 9053,
    "peers": ["10.0.0.12:9053", "10.0.0.13:9053"]
  }
}
```

**How sync works:**

1. Save a config change on **any** node.
2. That node bumps `config_version`, applies the change in memory, and pushes the full config to all online peers in < 1 s.
3. Nodes that were offline catch up within 30 s when they come back — no operator action required.
4. `config_version` is persisted to disk on every change so a restarted node rejoins at the correct version.

```bash
$ curl -s http://localhost:9053/cluster | python3 -m json.tool
{
  "this":  { "config_version": 12, "status": "healthy" },
  "peers": {
    "10.0.0.12:9053": { "config_version": 12, "status": "synced" },
    "10.0.0.13:9053": { "config_version": 12, "status": "synced" }
  }
}
```

| Scenario | Convergence time |
|----------|-----------------|
| Config saved on any online node | < 1 s |
| Node reboots and catches up | 10 – 40 s |
| Periodic background reconcile | <= 30 s |

### Multiple nodes on one machine

Useful for local testing or single-host HA setups:

```bash
nanodns start --port 5353 --mgmt-port 9053 --config node1.json
nanodns start --port 5354 --mgmt-port 9054 --config node2.json
nanodns start --port 5355 --mgmt-port 9055 --config node3.json
```

---

## Management API

Enable with `"mgmt_port": 9053` in config. **Bind `mgmt_host` to an internal interface only** — the API has no authentication.

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Liveness — 503 when unavailable |
| `/ready` | GET | Readiness — 503 until config loaded |
| `/metrics` | GET | Cache stats, query count, uptime, `config_version` |
| `/cluster` | GET | All peers with version and reachability |
| `/config/raw` | GET | Full config JSON (used by peer catch-up) |
| `/reload` | POST | Reload from disk, bump version, push to peers |
| `/sync` | POST | Accept versioned config push from a peer |

---

## Docker

```bash
# Generate a config first
nanodns init nanodns.json

# Start
docker compose up -d

# Test
dig @127.0.0.1 web.internal.lan A +short
```

```yaml
# docker-compose.yml
services:
  nanodns:
    image: ghcr.io/nanodns/nanodns:latest
    restart: unless-stopped
    ports:
      - "53:53/udp"
      - "9053:9053/tcp"
    volumes:
      - ./nanodns.json:/etc/nanodns/nanodns.json
    cap_add: [NET_BIND_SERVICE]
```

**Image details:**

- Tags: `latest` · `1.2.3` (pinned) · `sha-a1b2c3` (immutable commit pin)
- Platforms: `linux/amd64` · `linux/arm64` · `linux/arm/v7`
- Base: [Chainguard glibc-dynamic](https://images.chainguard.dev/) — distroless, non-root by default, minimal CVE surface

**Verify the image signature** (Sigstore cosign — no secret keys involved):

```bash
cosign verify \
  --certificate-identity-regexp="https://github.com/nanodns/nanodns/.github/workflows/release.yml@refs/tags/.*" \
  --certificate-oidc-issuer="https://token.actions.githubusercontent.com" \
  ghcr.io/nanodns/nanodns:latest
```

---

## systemd (Linux production)

```ini
[Unit]
Description=NanoDNS Server
After=network.target

[Service]
ExecStart=/usr/local/bin/nanodns start --config /etc/nanodns/nanodns.json
Restart=on-failure
RestartSec=5
AmbientCapabilities=CAP_NET_BIND_SERVICE
NoNewPrivileges=yes
ProtectSystem=strict
ReadOnlyPaths=/etc/nanodns

[Install]
WantedBy=multi-user.target
```

A pre-written `nanodns.service` file is included in every binary release archive.

---

## CLI reference

```
nanodns start   [--config FILE]    Path to config file (default: nanodns.json)
                [--host HOST]      Override DNS bind address
                [--port PORT]      Override DNS port
                [--mgmt-host HOST] Override management API bind address
                [--mgmt-port PORT] Override management API port  (0 = disabled)
                [--log-level LVL]  TRACE | DEBUG | INFO | WARN | ERROR
                [--no-cache]       Disable response cache

nanodns init    [OUTPUT]           Write an example config (default: nanodns.json)
nanodns check   CONFIG             Validate config and print a summary
nanodns --version
```

---

## Security

- **No shell, no package manager** in the container — Chainguard distroless base.
- **Memory-safe by construction** — Rust's ownership model eliminates buffer overflows, use-after-free, and data races at compile time.
- **Cosign-signed images** — every release binary and container image is signed with Sigstore keyless signing; verifiable without trusting a private key.
- **Minimal attack surface** — ~3 MB static binary, zero runtime dependencies, no dynamic linking.
- **Management API is unauthenticated** — firewall port 9053 from the public internet and bind `mgmt_host` to an internal interface.

---

## Contributing

```bash
git clone https://github.com/nanodns/nanodns
cd nanodns
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Commit prefix to CI behaviour:

| Prefix | Tests | Docker build | Release publish |
|--------|:-----:|:------------:|:---------------:|
| `feat` `fix` `perf` `refactor` | yes | yes on main | on tag |
| `test` `ci` `build` | yes | skip | skip |
| `docs` `style` `chore` | skip | skip | skip |

Bug reports, feature requests, and PRs are welcome.

---

## Project layout

```
src/
├── main.rs         CLI entry (clap): start / init / check
├── error.rs        Error types (thiserror)
├── config/         JSON config: load, validate, persist, write_example
├── dns/
│   ├── resolver.rs Local records -> rewrites -> zones -> upstream forward
│   ├── packet.rs   hickory-proto: build DNS wire-format responses
│   └── wildcard.rs Wildcard matching (*.foo.bar, single-level)
├── cache/          TTL-aware LRU response cache
├── server/         tokio UDP loop, hot-reload watcher, shared AppState
├── mgmt/           axum HTTP API (7 endpoints)
└── sync/           Peer version probing + 30 s reconcile loop
```

---

## License

[MIT](LICENSE)

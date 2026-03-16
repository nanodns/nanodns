# ── Build stage ──────────────────────────────────────────────────────────────
FROM rust:1.77-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
# Cache dependencies by building a dummy main first
RUN mkdir src && echo "fn main(){}" > src/main.rs && cargo build --release 2>/dev/null; rm -rf src

COPY src ./src
RUN cargo build --release

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM scratch

COPY --from=builder /app/target/release/nanodns /nanodns
COPY nanodns.json /etc/nanodns/nanodns.json

# DNS (UDP) + management API (TCP)
EXPOSE 53/udp 9053/tcp

ENTRYPOINT ["/nanodns"]
CMD ["start", "--config", "/etc/nanodns/nanodns.json"]

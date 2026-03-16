# ── Build stage — Chainguard Rust (hardened, minimal CVE surface) ─────────────
ARG PACKAGE=nanodns

FROM chainguard/rust:latest AS build

WORKDIR /app

# Cache dependency layer: compile a stub first so source changes don't
# invalidate the (expensive) dependency build.
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main(){}" > src/main.rs \
    && cargo build --release 2>/dev/null; rm -rf src

# Real build
COPY src ./src
RUN cargo build --release && strip target/release/nanodns

# ── Runtime stage — Chainguard glibc-dynamic (distroless, nonroot) ────────────
FROM chainguard/glibc-dynamic:latest

ARG PACKAGE=nanodns

LABEL org.opencontainers.image.source="https://github.com/nanodns/nanodns" \
      org.opencontainers.image.description="Lightweight DNS server for internal networks" \
      org.opencontainers.image.licenses="MIT"

COPY --from=build --chown=nonroot:nonroot \
     /app/target/release/${PACKAGE} /usr/local/bin/${PACKAGE}

# Config is mounted at runtime:
#   docker run -v ./nanodns.json:/etc/nanodns/nanodns.json ...
# Generate a starter config with:
#   docker run --rm nanodns init /tmp/nanodns.json

# DNS (UDP) + management API (TCP)
EXPOSE 53/udp 9053/tcp

CMD ["/usr/local/bin/nanodns", "start", "--config", "/etc/nanodns/nanodns.json"]

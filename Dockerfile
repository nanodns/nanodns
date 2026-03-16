# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1.77-alpine AS builder

ARG VERSION=dev
ARG TARGETPLATFORM

RUN apk add --no-cache musl-dev

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./

# Cache dependencies with a dummy main
RUN mkdir src && echo "fn main(){}" > src/main.rs \
    && cargo build --release 2>/dev/null; rm -rf src

COPY src ./src

# Patch version if provided
RUN if [ "$VERSION" != "dev" ]; then \
      sed -i "s/^version = \".*\"/version = \"${VERSION#v}\"/" Cargo.toml; \
    fi

RUN cargo build --release && strip target/release/nanodns

# ── Runtime stage — zero-OS scratch image ────────────────────────────────────
FROM scratch

LABEL org.opencontainers.image.source="https://github.com/iyuangang/nanodns" \
      org.opencontainers.image.description="Lightweight DNS server for internal networks" \
      org.opencontainers.image.licenses="MIT"

COPY --from=builder /app/target/release/nanodns /nanodns
# Config is mounted at runtime via -v ./nanodns.json:/etc/nanodns/nanodns.json
# Use: nanodns init  to generate a starter config

EXPOSE 53/udp 9053/tcp

ENTRYPOINT ["/nanodns"]
CMD ["start", "--config", "/etc/nanodns/nanodns.json"]

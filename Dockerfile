# Spindle Dockerfile — single image for server + worker
# Usage:
#   Build:  docker build -t spindle:0.1.0 .
#   Server: docker run --rm -p 3000:3000 --mount type=bind,source=./spindle.toml,target=/config/spindle.toml spindle:0.1.0 server
#   Worker: docker run --rm --mount type=bind,source=./spindle.toml,target=/config/spindle.toml spindle:0.1.0 worker

FROM rust:1.97 AS builder

WORKDIR /build

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
COPY spindle-config/ ./spindle-config/
COPY spindle-server/ ./spindle-server/
COPY spindle-worker/ ./spindle-worker/
COPY spindle-cli/ ./spindle-cli/
COPY spindle-store/ ./spindle-store/
COPY spindle-signing/ ./spindle-signing/
COPY spindle-archive/ ./spindle-archive/
COPY spindle-rawarchive/ ./spindle-rawarchive/
COPY migrations/ ./migrations/

RUN cargo fetch

# Copy source and build
COPY . .
RUN cargo build --release --bin spindle-server --bin spindle-worker --bin spindle

# ── Runtime stage ──────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

LABEL org.opencontainers.image.title="spindle" \
      org.opencontainers.image.description="Spindle data platform — server, worker, and CLI" \
      org.opencontainers.image.version="1.0"

# Install CA certificates
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd --create-home --shell /bin/bash spindle

# Copy binaries from builder
COPY --from=builder /build/target/release/spindle-server /usr/local/bin/spindle-server
COPY --from=builder /build/target/release/spindle-worker /usr/local/bin/spindle-worker
COPY --from=builder /build/target/release/spindle /usr/local/bin/spindle

# Create config directory
RUN mkdir -p /config /var/lib/spindle && \
    chown -R spindle:spindle /config /var/lib/spindle

USER spindle

# Default to config at /config/spindle.toml
ENV SPINDLE_CONFIG=/config/spindle.toml

EXPOSE 3000

# Default entry point — can be overridden with: docker run spindle:0.1.0 <command>
# server:  HTTP API + ingest (port 3000)
# worker: queue consumer + rollups + exports
# spindle: CLI (operator)
ENTRYPOINT ["spindle-server"]
CMD ["--config", "/config/spindle.toml"]

# syntax=docker/dockerfile:1
# EasyBot — Multi-stage Docker Build
#
# Build:
#   docker build -t easybot .
#
# Run:
#   docker run -p 8080:8080 -v ./gateway.yaml:/etc/easybot/gateway.yaml easybot
#
# The image builds --features full to include all IM platform adapters.
# Configure via mounted gateway.yaml + environment variables for secrets.

# ── Builder Stage ──
FROM rust:slim-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy source
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY bin/ ./bin/

# Test crates are workspace members — copy them so Cargo can resolve the
# workspace. Since we only build --bin easybot, the test source won't be
# compiled (it is not in the dependency tree).
COPY tests/ ./tests/

# Build (use --mount for cache persistence across builds)
# --features "full,plugin-system" 启用所有内置适配器 + 插件系统
RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --locked --release --features "full,plugin-system" --bin easybot && \
    cp target/release/easybot /easybot

# ── Runtime Stage ──
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy the compiled binary
COPY --from=builder /easybot /usr/local/bin/easybot

# Create data directory
RUN mkdir -p /var/lib/easybot/data /var/lib/easybot/logs /var/lib/easybot/plugins /etc/easybot

# Expose API port
EXPOSE 8080

# Entry point — same CLI as standalone
ENTRYPOINT ["easybot"]
CMD ["--config", "/etc/easybot/gateway.yaml"]

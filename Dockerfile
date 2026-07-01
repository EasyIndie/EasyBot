# syntax=docker/dockerfile:1
# EasyBot — Multi-stage Docker Build
FROM rust:slim-bookworm AS builder

# Cargo retry settings for transient network errors (e.g. crates.io HTTP/2 resets)
ENV CARGO_NET_RETRY=5 \
    CARGO_HTTP_TIMEOUT=120 \
    CARGO_HTTP_MULTIPLEXING=false

WORKDIR /app
RUN apt-get update && apt-get install -y \
    pkg-config \
    protobuf-compiler \
    curl \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY bin/ ./bin/
COPY tests/ ./tests/
RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --locked --release --features "full,plugin-system" --bin easybot && \
    cp target/release/easybot /easybot

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -r -m -s /bin/bash easybot \
    && mkdir -p /var/lib/easybot/data /var/lib/easybot/logs /var/lib/easybot/plugins /etc/easybot \
    && chown -R easybot:easybot /var/lib/easybot /etc/easybot
COPY --from=builder /easybot /usr/local/bin/easybot
USER easybot
EXPOSE 8080
ENTRYPOINT ["easybot"]
CMD ["--config", "/etc/easybot/gateway.yaml"]

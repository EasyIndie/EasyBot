# syntax=docker/dockerfile:1
# EasyBot — Multi-stage Docker Build
FROM rust:slim-bookworm AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y \
    pkg-config \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY bin/ ./bin/
RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --locked --release --features "full,plugin-system" --bin easybot && \
    cp target/release/easybot /easybot

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /easybot /usr/local/bin/easybot
RUN mkdir -p /var/lib/easybot/data /var/lib/easybot/logs /var/lib/easybot/plugins /etc/easybot
EXPOSE 8080
ENTRYPOINT ["easybot"]
CMD ["--config", "/etc/easybot/gateway.yaml"]

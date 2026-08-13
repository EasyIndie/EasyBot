# syntax=docker/dockerfile:1
# EasyBot — Multi-stage Docker Build
FROM rust:slim-bookworm@sha256:cfbb0e0ef7a73e736386bfa346f1cb0503c6d162969dc9426fb37834f3f64c25 AS builder

WORKDIR /app
RUN apt-get -o Acquire::Retries=5 update && apt-get -o Acquire::Retries=5 install -y \
    pkg-config \
    curl \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Cargo retry settings for transient network errors (e.g. crates.io HTTP/2 resets).
ENV CARGO_NET_RETRY=5 \
    CARGO_HTTP_TIMEOUT=120 \
    CARGO_HTTP_MULTIPLEXING=false
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY bin/ ./bin/
# 插件入门示例是 workspace 成员（workspace 解析需其清单；--bin easybot 不编译它）
COPY plugins/example-hello-adapter/Cargo.toml plugins/example-hello-adapter/
COPY plugins/example-hello-adapter/src/ plugins/example-hello-adapter/src/
# Only Cargo.toml + minimal stubs for workspace member resolution (--bin easybot skips test compilation)
COPY tests/plugins/mock-adapter/Cargo.toml tests/plugins/mock-adapter/
COPY tests/plugins/mock-adapter/src/ tests/plugins/mock-adapter/src/
COPY tests/integration/Cargo.toml tests/integration/
COPY tests/integration/src/ tests/integration/src/
COPY tests/e2e/Cargo.toml tests/e2e/
COPY tests/e2e/src/ tests/e2e/src/
COPY tests/fixtures/Cargo.toml tests/fixtures/
COPY tests/fixtures/src/ tests/fixtures/src/
RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --locked --release --features "default,plugin-system" --bin easybot && \
    cp target/release/easybot /easybot

FROM debian:bookworm-slim@sha256:60eac759739651111db372c07be67863818726f754804b8707c90979bda511df
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -r -u 10001 --user-group -m -s /bin/bash easybot \
    && mkdir -p /var/lib/easybot/data /var/lib/easybot/logs /var/lib/easybot/plugins /etc/easybot \
    && chown -R easybot:easybot /var/lib/easybot /etc/easybot
COPY --from=builder --chown=easybot:easybot /easybot /usr/local/bin/easybot
# 内置容器化默认配置（server.host: 0.0.0.0，保证 -p 端口发布可达）。
# 用户挂载自己的 gateway.yaml 时会覆盖此文件。
COPY --chown=easybot:easybot deploy/gateway.container.yaml /etc/easybot/gateway.yaml
USER easybot
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --retries=3 CMD ["curl", "-f", "http://127.0.0.1:8080/api/v1/live"]
ENTRYPOINT ["easybot"]
CMD ["--config", "/etc/easybot/gateway.yaml"]

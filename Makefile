# EasyBot 开发工具

.DEFAULT_GOAL := help

.PHONY: help setup verify verify-fast test test-full lint fmt \
        run run-init run-fresh watch check clean

help:  ## 显示此帮助
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

# ── 项目初始化 ────────────────────────────────

setup:  ## 首次克隆后：配置 git hooks 路径
	git config core.hooksPath scripts
	@echo "✅ Git hooks 已配置 (core.hooksPath = scripts)"

# ── CI / 验收 ─────────────────────────────────

verify:  ## 运行完整验收（与 CI 一致）
	bash scripts/verify.sh

verify-fast:  ## 快速验收（跳过 clippy + fmt）
	bash scripts/verify.sh --fast

test:  ## 运行所有测试
	cargo test --workspace

test-full:  ## 运行全部测试（含 plugin-system feature）
	cargo test --workspace --features "full,plugin-system"

# ── 代码质量 ──────────────────────────────────

lint:  ## 代码规范检查（fmt + clippy）
	cargo fmt --all --check
	cargo clippy --workspace --features "full,plugin-system" --all-targets -- -D warnings

fmt:  ## 自动格式化代码
	cargo fmt --all

check:  ## 快速编译检查
	cargo check

# ── 本地开发 ──────────────────────────────────

DEBUG_FLAG ?= --debug

run:  ## 编译并启动（默认 --debug，make run DEBUG= 可去掉）
	cargo run $(DEBUG_FLAG)

run-init:  ## 初始化隔离目录后启动（不影响 ~/.easybot/）
	@test -d /tmp/easybot-dev || cargo run -- --dir /tmp/easybot-dev --init
	cargo run -- --dir /tmp/easybot-dev $(DEBUG_FLAG)

run-fresh:  ## 清理隔离目录后全新初始化并启动
	rm -rf /tmp/easybot-dev
	cargo run -- --dir /tmp/easybot-dev --init
	cargo run -- --dir /tmp/easybot-dev $(DEBUG_FLAG)

watch:  ## Watch 模式：改代码自动重编重启（需 cargo install cargo-watch）
	cargo watch -x 'run -- --debug'

clean:  ## 清理编译产物
	cargo clean

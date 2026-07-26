# EasyBot 开发工具

.DEFAULT_GOAL := help

CARGO ?= cargo
FEATURES ?= default,plugin-system

.PHONY: help setup verify verify-push check lint fmt test run clean

help:  ## 显示此帮助
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

# ── 环境 ─────────────────────────────────────

setup:  ## 初始化本地开发环境
	git config core.hooksPath .githooks

# ── Hook / CI 验收 ───────────────────────────

verify:  ## 运行完整验收（与 CI 一致）
	bash scripts/verify.sh

verify-push:  ## push 前门禁（clippy + build + test，快于完整验收）
	bash scripts/verify-push.sh

# ── 代码质量 ──────────────────────────────────

check:  ## 快速编译检查（同 pre-commit）
	$(CARGO) fmt --all --check
	$(CARGO) check --workspace --features "$(FEATURES)"

fmt:  ## 自动格式化代码
	$(CARGO) fmt --all

lint:  ## 代码规范检查（fmt + clippy）
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --features "$(FEATURES)" --all-targets -- -D warnings

test:  ## 运行所有测试
	$(CARGO) test --workspace --features "$(FEATURES)"

# ── 本地开发 ──────────────────────────────────

DEBUG_FLAG ?= --debug

run:  ## 编译并启动（默认 --debug，make run DEBUG= 可去掉）
	$(CARGO) run -- $(DEBUG_FLAG)

clean:  ## 清理编译产物
	$(CARGO) clean

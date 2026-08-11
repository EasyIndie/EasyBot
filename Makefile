# EasyBot 开发工具

.DEFAULT_GOAL := help

CARGO ?= cargo
FEATURES ?= default,plugin-system

.PHONY: help setup verify verify-push check check-package lint fmt test test-package run diagnostics benchmark ci-history disk clean clean-deep

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

check:  ## workspace 格式与快速编译检查
	$(CARGO) fmt --all --check
	$(CARGO) check --workspace --features "$(FEATURES)"

check-package:  ## 检查单个包（用法: make check-package PACKAGE=easybot-core）
	@test -n "$(PACKAGE)" || (echo "ERROR: 请设置 PACKAGE=<cargo-package>" >&2; exit 2)
	$(CARGO) check -p "$(PACKAGE)" $(if $(PACKAGE_FEATURES),--features "$(PACKAGE_FEATURES)",)

fmt:  ## 自动格式化代码
	$(CARGO) fmt --all

lint:  ## 代码规范检查（fmt + clippy）
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --features "$(FEATURES)" --all-targets -- -D warnings

test:  ## 运行所有测试
	$(CARGO) test --workspace --features "$(FEATURES)"

test-package:  ## 测试单个包（用法: make test-package PACKAGE=easybot-core）
	@test -n "$(PACKAGE)" || (echo "ERROR: 请设置 PACKAGE=<cargo-package>" >&2; exit 2)
	$(CARGO) test -p "$(PACKAGE)" $(if $(PACKAGE_FEATURES),--features "$(PACKAGE_FEATURES)",)

# ── 本地开发 ──────────────────────────────────

DEBUG_FLAG ?= --debug

run:  ## 编译并启动（默认 --debug，make run DEBUG_FLAG= 可去掉）
	$(CARGO) run --features "$(FEATURES)" -- $(DEBUG_FLAG)

diagnostics:  ## 只读报告：工具链、worktree、Cargo/Docker/构建占用
	bash scripts/dev-diagnostics.sh

benchmark:  ## 热缓存开发基线；COLD=1 时先清当前 target
	bash scripts/dev-benchmark.sh $(if $(filter 1,$(COLD)),--cold,)

ci-history:  ## 最近 10 次 PR CI 的排队与墙钟 P50/P95（只读，需要 gh）
	bash scripts/ci-history.sh

disk:  ## 显示可回收空间与安全清理选项（不删除）
	bash scripts/dev-disk.sh

clean:  ## 清理编译产物
	$(CARGO) clean

clean-deep:  ## 深度清理预览；按脚本提示显式选择并确认
	bash scripts/dev-disk.sh --help

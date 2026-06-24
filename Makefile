# EasyBot 开发工具

.PHONY: setup hooks test lint check clean

# 首次克隆后运行：配置 git hooks 路径
setup:
	git config core.hooksPath scripts
	@echo "✅ Git hooks 已配置 (core.hooksPath = scripts)"

# 运行完整验收（与 CI 一致）
verify:
	bash scripts/verify.sh

# 快速验收（跳过 clippy + fmt）
verify-fast:
	bash scripts/verify.sh --fast

# 运行所有测试
test:
	cargo test --workspace

test-full:
	cargo test --workspace --features "full,plugin-system"

# 代码检查
lint:
	cargo fmt --all --check
	cargo clippy --workspace --features "full,plugin-system" --all-targets -- -D warnings

# 格式化代码
fmt:
	cargo fmt --all

# 清理编译产物
clean:
	cargo clean

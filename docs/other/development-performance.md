# 开发与 CI 性能指南

## 验证分层

日常开发按反馈速度分为三层：

1. `pre-commit` 仅在暂存 Rust 文件时执行 `cargo fmt --check`，不触发编译。
2. `make verify-push` 执行全 workspace clippy、CLI/mock 测试前置产物和全特性测试。
3. `make verify` 执行 12 步完整验收，包括 feature 组合与发布/恢复门禁。

局部改动可使用 `make check-package PACKAGE=easybot-core` 或
`make test-package PACKAGE=easybot-core`。如需 package feature，追加
`PACKAGE_FEATURES=plugin-system`。以下改动必须回退到 workspace 验证：

- 根 `Cargo.toml`、`Cargo.lock`、构建脚本或 CI；
- `easybot-core`、公共 API/fixture 或跨多个 crate 的接口；
- feature 定义、条件编译和插件 ABI。

## 性能基线

`make diagnostics` 是只读报告，显示工具链、worktree、当前 `target`、Cargo、
Rustup 和 Docker 可回收空间。`make benchmark` 记录热缓存 workspace check、
核心测试和 pre-push；`make benchmark COLD=1` 会先用 `cargo clean` 删除当前
worktree 的 `target`，用于明确执行的冷基线。

比较优化前后数据时应使用相同提交、工具链和 feature，冷/热各测一轮。
CI 关键编译步骤会把耗时、`target` 大小和 Rust cache 命中状态写入 Job Summary；
`make ci-history` 汇总最近 10 次 PR CI 的排队与墙钟 P50/P95。以连续 10 次
同类 PR 判断趋势，runner 排队时间应与执行时间分开。

## 磁盘治理

`make disk` 默认只报告，不删除任何内容。当前 `target` 达到 8 GiB 时会提示；可用
`EASYBOT_TARGET_BUDGET_GIB` 覆盖预算。
可执行操作必须明确指定范围并传入 `--yes`：

```bash
bash scripts/dev-disk.sh --clean-target --yes
bash scripts/dev-disk.sh --prune-worktrees --yes
bash scripts/dev-disk.sh --prune-docker --yes
bash scripts/dev-disk.sh --clean-cargo --yes
```

- `--clean-target` 只清当前 worktree 的 Cargo 产物，后续会发生冷编译。
- `--prune-worktrees` 只清 Git 已判定失效的 worktree 元数据，不删除活跃目录。
- `--prune-docker` 只清理七天前的 BuildKit 构建缓存，不删除镜像或数据卷。
- `--clean-cargo` 仅在已安装 `cargo-cache` 时执行其保守 autoclean。

Docker Desktop 数据文件的表观大小并不等于可回收空间，以 `docker system df`
的 Reclaimable 数据为准。项目不自动定期清理，也不在多个 worktree 之间共享
`target`，以保留热缓存并避免 Cargo 锁争用。

## CI 触发边界

- 普通 PR：主测试、feature 检查和六平台矩阵；不运行 coverage 或 Docker release 构建。
- 容器/部署相关 PR：额外运行 Docker 镜像验证。
- main：运行主 CI 和 coverage；符合镜像路径条件时构建双架构镜像。
- coverage 另在每周一运行，可手动触发；安全审计与灾备门禁保持独立。

CI 的 protoc 固定为官方 v23.4 发布包，并在各 runner 上校验 SHA-256 后安装。
不依赖 Homebrew、apt 或 Node.js setup action，以避免 runner 全局状态警告和
JavaScript action 运行时弃用影响。

# Contributing to EasyBot

Thank you for your interest in contributing to EasyBot! This document will help you get started.

## Development Environment Setup

### Prerequisites

- [Rust](https://rustup.rs/) (stable, 1.94+)
- Git
- (Optional) Docker for containerized development

### Getting Started

```bash
# Clone the repository
git clone https://github.com/EasyIndie/EasyBot.git
cd EasyBot

# Run setup (configures git hooks)
make setup

# Build
cargo build

# Run tests
cargo test

# Run with debug logging
cargo run -- --debug
```

### Initialize Configuration

```bash
# Create default config directory (~/.easybot/)
cargo run -- --init

# Or specify a custom directory
cargo run -- --init --dir /path/to/config
```

## Code Style

- **Format**: `cargo fmt` (enforced by CI)
- **Lint**: `cargo clippy --all-targets` (enforced by CI)
- **Commit messages**: Follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`)

### Pre-commit / Pre-push Hooks

The project uses git hooks to enforce quality:

- **Pre-commit**: Runs `cargo fmt --check` on staged `.rs` files.
- **Pre-push**: Runs `verify.sh` which does clippy checks, builds, and runs full test suite.

Run `make setup` to automatically configure your local repository to use these hooks.

## Architecture

See [`CLAUDE.md`](CLAUDE.md) for the full architecture overview. Key points:

```
API Layer (easybot-api) → Core Logic (easybot-core) → Adapter Layer (easybot-adapter-*)
```

- **Core types** are in `crates/easybot-core/src/types/`
- **Adapters** implement the `PlatformAdapter` trait
- **API routes** are in `crates/easybot-api/src/routes/`

## Adding a New Adapter

1. Create a new crate `crates/easybot-adapter-<platform>/`
2. Implement the `PlatformAdapter` trait from `easybot-core`
3. Register the adapter factory in `bin/src/main.rs` in `register_builtin_adapters()`
4. Add tests (mock + E2E)
5. Add a feature flag in the root `Cargo.toml`

See existing adapters (`easybot-adapter-telegram`, `easybot-adapter-discord`, etc.) for reference.

## Testing

```bash
# Run all unit tests
cargo test --lib

# Run tests for a specific crate
cargo test -p easybot-core
cargo test -p easybot-adapter-telegram

# Run integration tests
cargo build -p ipc-mock-plugin && cargo test -p integration-tests

# Run E2E tests
cargo test -p e2e-tests
```

### Test Patterns

- **Unit tests**: In `#[cfg(test)] mod tests` at the bottom of each source file.
- **Mock tests**: In `tests/send_mock.rs` under each adapter crate. Use `wiremock` for HTTP mocking.
- **E2E tests**: In `tests/e2e/tests/`. These spawn the full gateway with mock servers.
- **Integration tests**: In `tests/integration/`. These test the plugin system.

## Pull Request Process

1. Fork the repository and create a feature branch.
2. Make your changes, following the code style guidelines.
3. Add tests for new functionality.
4. Ensure all tests pass: `cargo test --workspace --features "default,plugin-system"`
5. Run clippy: `cargo clippy --all-targets --features "default,plugin-system"`
6. Run format check: `cargo fmt --all -- --check`
7. Update documentation if needed.
8. Submit a pull request.

## Merge Queue & Auto-merge

`main` is protected by a ruleset that requires the full status-check set **and a merge queue** (squash). CI runs on a temporary `merge_group` ref and the queue squash-merges the batch once everything is green; nothing is pushed directly to `main`.

### Solo maintainer（当前状态）

GitHub 不允许 PR 作者审批自己的 PR，所以在单人维护阶段分支保护的 **required approvals 设为 0**。此时仓库自带的 auto-merge 能端到端工作：

```bash
gh pr merge --auto --squash <pr>    # 之后 CI 一绿就自动进 merge queue 并合入
```

`Dependabot auto-merge` 工作流已对 patch/minor 依赖升级自动执行这一步（major 升级仍保持手动）。

> 实测：把 required approvals 设为 0 后，PR 的 `autoMergeRequest` 会在 checks 全绿时自动入队（`mergeQueueEntry: AWAITING_CHECKS`）并合入。此前 auto-merge 看起来“卡住”只是因为单人无法满足 required review。

### 与其他开发者协作时

一旦有第二个人能评审，恢复评审门禁：

```bash
gh api -X PATCH repos/EasyIndie/EasyBot/branches/main/protection/required_pull_request_reviews \
  -F required_approving_review_count=1
```

- 给新维护者 **Write**（或 **Maintain**）权限；之后由其审批，auto-merge 会在「审批通过 + required checks 全绿」时自动入队。
- 可选：加 `CODEOWNERS` 并开启 *Require review from Code Owners*。
- Dependabot PR 此时需要人工审批；若希望它们继续自动合并，加一个 auto-approve 步骤（建议用 **GitHub App** 或 **PAT**，不要依赖默认 `GITHUB_TOKEN` 的审批）。
- 可选更严：`dismiss_stale_reviews=true`、`require_last_push_approval=true`。

### 排查 auto-merge 不生效

1. `gh pr view <n> --json mergeStateStatus,reviewDecision`：`BLOCKED` + `REVIEW_REQUIRED` 表示评审门禁未满足（单人时改 required approvals=0）。
2. `gh pr view <n> --json statusCheckRollup`：任一 required check 失败/缺失都不会入队。
3. `strict_required_status_checks_policy` 要求分支为最新；过旧请先 rebase（Dependabot 会自动 rebase）。

## Feature Flags

| Flag | Enables |
|------|---------|
| `default` | All 5 built-in adapters (Telegram, Discord, 飞书, QQ, WeChat) |
| `default` | All 5 built-in adapters (Telegram, Discord, 飞书, QQ, WeChat) |
| `adapter-telegram` | Telegram Bot API adapter |
| `adapter-discord` | Discord Gateway adapter |
| `adapter-feishu` | 飞书/Lark adapter |
| `adapter-qq` | QQ Bot adapter |
| `adapter-wechat` | WeChat iLink Bot adapter |
| `plugin-system` | Dynamic plugin loading |

## Questions?

Open an issue on GitHub or start a discussion.

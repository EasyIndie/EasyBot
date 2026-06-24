# Contributing to EasyBot

Thank you for your interest in contributing to EasyBot! This document will help you get started.

## Development Environment Setup

### Prerequisites

- [Rust](https://rustup.rs/) (stable, 1.75+)
- Git
- (Optional) Docker for containerized development

### Getting Started

```bash
# Clone the repository
git clone https://github.com/wangzhizhou/EasyBot.git
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
cargo build -p mock-adapter && cargo test -p integration-tests

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
4. Ensure all tests pass: `cargo test --features full`
5. Run clippy: `cargo clippy --all-targets --features full`
6. Run format check: `cargo fmt --all -- --check`
7. Update documentation if needed.
8. Submit a pull request.

## Feature Flags

| Flag | Enables |
|------|---------|
| `full` | All built-in adapters + plugin system |
| `adapter-telegram` | Telegram Bot API adapter |
| `adapter-discord` | Discord Gateway adapter |
| `adapter-feishu` | 飞书/Lark adapter |
| `adapter-qq` | QQ Bot adapter |
| `adapter-wechat` | WeChat iLink Bot adapter |
| `plugin-system` | Dynamic plugin loading |

## Questions?

Open an issue on GitHub or start a discussion.

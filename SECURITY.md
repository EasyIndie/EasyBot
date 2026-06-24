# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Instead, please report them via email to the project maintainer. You should receive a response within 48 hours. If the issue is confirmed, a patch will be released as soon as possible.

### What to Include

- A description of the vulnerability and its potential impact
- Steps to reproduce or a proof-of-concept
- The affected version(s)

### Process

1. Submit your report via email.
2. You will receive an acknowledgment within 48 hours.
3. We will investigate and keep you informed of progress.
4. Once a fix is ready, we will coordinate the release and disclosure timeline.

## Security Expectations

EasyBot is an IM Gateway that handles credentials for multiple messaging platforms. Please be mindful of:

- **API Keys**: Store them in `.env` files (chmod 600) and never commit them to version control.
- **Configuration**: Use `gateway.local.yaml` for sensitive overrides (it is `.gitignore`-d).
- **TLS**: In production, always use TLS (via reverse proxy nginx/caddy/traefik or direct TLS config).
- **Rate Limiting**: Enabled by default — keep it enabled for production deployments.
- **Docker**: The Docker image runs as a non-root `easybot` user. Do not override `USER` in production.

### Plugin Sandbox

EasyBot supports third-party adapter plugins loaded as native shared libraries (`.so`/`.dylib`/`.dll`). This inherently grants plugins full access to the host process memory, file system, and network.

**Risks:**
- A malicious plugin can read all credentials, environment variables, and configuration
- A plugin can access the SQLite database file and message history
- A plugin has unrestricted network access
- A plugin can manipulate the process state (e.g., EventBus events)

**Mitigations currently implemented:**
- Plugin manifest path validation: absolute paths and `..` traversal are rejected (`plugin/manifest.rs:66-76`)
- Plugins are loaded from `~/.easybot/plugins/` directory only (not arbitrary paths)
- Plugin `unsafe_code` is denied via workspace lint (`[lints.rust] unsafe_code = "deny"`)

**Recommended best practices for operators:**
1. Only install plugins from trusted sources
2. Run EasyBot in a container or sandboxed environment (e.g., Docker with `--read-only` rootfs, seccomp profile)
3. Use a dedicated OS user for the EasyBot process with minimal permissions
4. Restrict plugin directory permissions (`chmod 700 ~/.easybot/plugins/`)
5. Audit plugin source code before deployment
6. Consider using WebAssembly-based plugin sandboxing for untrusted plugins (future roadmap)

**Future improvements planned:**
- WASM-based plugin runtime with capability-based security model
- Plugin permission manifest (e.g., `plugin.yaml` with `permissions: [network, filesystem, secrets]`)
- Plugin signature verification

### Known Security Considerations

- The `--debug` flag creates a development API key with `["*"]` permissions. Never use `--debug` in production.
- WebSocket connections are authenticated via token, but the connection itself is not TLS-encrypted unless TLS is configured at the reverse proxy level.
- Metrics endpoint (`/metrics`) requires Bearer token authentication.

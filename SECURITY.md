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

### Known Security Considerations

- The `--debug` flag creates a development API key with `["*"]` permissions. Never use `--debug` in production.
- WebSocket connections are authenticated via token, but the connection itself is not TLS-encrypted unless TLS is configured at the reverse proxy level.
- Metrics endpoint (`/metrics`) is public by default. If your metrics contain sensitive data, protect this endpoint at the reverse proxy level.

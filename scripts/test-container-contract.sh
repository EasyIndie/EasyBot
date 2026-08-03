#!/usr/bin/env bash
set -euo pipefail

ROOT=$(git rev-parse --show-toplevel)
cd "$ROOT"

expected='http://127.0.0.1:8080/api/v1/live'
for file in Dockerfile Dockerfile.release; do
  grep -Fq "HEALTHCHECK" "$file" || {
    echo "$file has no container healthcheck" >&2
    exit 1
  }
  grep -Fq "$expected" "$file" || {
    echo "$file healthcheck must use the liveness endpoint: $expected" >&2
    exit 1
  }
  grep -Eq 'useradd -r -u 10001 --user-group|addgroup -S -g 10001 easybot' "$file" || {
    echo "$file must create the fixed non-root runtime UID 10001" >&2
    exit 1
  }
  if grep -Eq 'HEALTHCHECK.*localhost:8080/(health|api/v1/health)([" /]|$)' "$file"; then
    echo "$file healthcheck uses a diagnostic endpoint instead of liveness" >&2
    exit 1
  fi
  while IFS= read -r base; do
    [[ "$base" =~ @sha256:[0-9a-f]{64}$ ]] || {
      echo "$file contains a mutable base image reference: $base" >&2
      exit 1
    }
  done < <(awk '$1 == "FROM" { print $2 }' "$file")
done

grep -Fq "$expected" docker-compose.yml || {
  echo "docker-compose.yml healthcheck must use the liveness endpoint" >&2
  exit 1
}
grep -Fq 'wget, -q, -O, /dev/null, http://127.0.0.1:8080/api/v1/live' deploy/docker-compose.production.yml || {
  echo "production compose healthcheck must use wget because the release runtime is Alpine without curl" >&2
  exit 1
}

for variable in EASYBOT_ADMIN_PASSWORD DATABASE_URL TELEGRAM_BOT_TOKEN \
  DISCORD_BOT_TOKEN FEISHU_APP_ID FEISHU_APP_SECRET QQ_APP_ID QQ_CLIENT_SECRET; do
  for optional_secret in "$variable" "${variable}_FILE"; do
    grep -Eq "^[[:space:]]+- ${optional_secret}$" docker-compose.yml || {
      echo "docker-compose.yml must pass through ${optional_secret} only when set" >&2
      exit 1
    }
    if grep -Eq "^[[:space:]]+- ${optional_secret}=" docker-compose.yml; then
      echo "docker-compose.yml must not inject an empty ${optional_secret}" >&2
      exit 1
    fi
  done
done

while IFS= read -r image; do
  [[ "$image" =~ @sha256:[0-9a-f]{64}$ ]] || {
    echo "docker-compose.yml contains a mutable service image: $image" >&2
    exit 1
  }
done < <(awk '$1 == "image:" { print $2 }' docker-compose.yml)

for expected_proxy_setting in \
  'Strict-Transport-Security "max-age=31536000; includeSubDomains"' \
  'X-Content-Type-Options "nosniff"' \
  'health_uri /api/v1/live' \
  'reverse_proxy easybot:8080' \
  'request>uri delete' \
  'request>headers delete' \
  'request>remote_ip ip_mask 24 48' \
  'request>client_ip ip_mask 24 48'; do
  grep -Fq "$expected_proxy_setting" deploy/Caddyfile || {
    echo "deploy/Caddyfile is missing: $expected_proxy_setting" >&2
    exit 1
  }
done
grep -Fq 'production-tls' docker-compose.yml
grep -Fq 'EASYBOT_DOMAIN=' docker-compose.yml
grep -Fq 'pids: 512' docker-compose.yml || {
  echo "docker-compose.yml must align the inherited PID limit with production" >&2
  exit 1
}
for compose_file in docker-compose.yml deploy/docker-compose.production.yml; do
  grep -Fq '/tmp:size=16M' "$compose_file" || {
    echo "$compose_file must provide writable tmpfs for Caddy temporary files" >&2
    exit 1
  }
done
grep -Fq 'image: ${EASYBOT_IMAGE:?' deploy/docker-compose.production.yml
grep -Fq '${EASYBOT_DATA_DIR:?' deploy/docker-compose.production.yml
grep -Fq './gateway.production.local.yaml:/var/lib/easybot/gateway.local.yaml:ro' deploy/docker-compose.production.yml
grep -Fq 'host: "0.0.0.0"' deploy/gateway.production.local.yaml
[ "$(grep -Fc 'security_opt: !override [no-new-privileges:true]' deploy/docker-compose.production.yml)" = 2 ] || {
  echo "production compose must replace inherited security_opt lists" >&2
  exit 1
}
if grep -Fq 'build:' deploy/docker-compose.production.yml; then
  echo "production compose must never build an unverified local image" >&2
  exit 1
fi
for runtime_policy in 'pids_limit:' 'mem_limit:' 'cpus:' 'stop_grace_period: 45s' 'max-size:' 'max-file:'; do
  grep -Fq "$runtime_policy" deploy/docker-compose.production.yml || {
    echo "production compose is missing runtime policy: $runtime_policy" >&2
    exit 1
  }
done
for writable_runtime_dir in \
  '/var/lib/easybot/logs:size=64M' \
  '/var/lib/easybot/plugins:size=16M' \
  '/var/lib/easybot/certs:size=16M' \
  '/var/lib/easybot/secrets:size=16M'; do
  for compose_file in docker-compose.yml deploy/docker-compose.production.yml; do
    grep -Fq "$writable_runtime_dir" "$compose_file" || {
      echo "$compose_file must provide writable tmpfs for: $writable_runtime_dir" >&2
      exit 1
    }
  done
done

echo "Container healthcheck contract passed"

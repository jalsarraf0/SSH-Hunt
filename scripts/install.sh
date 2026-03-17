#!/usr/bin/env bash
set -euo pipefail

ROOT="/docker/ssh-hunt"
mkdir -p "$ROOT" "$ROOT/volumes/postgres" "$ROOT/volumes/ssh-hunt" "$ROOT/volumes/backups" "$ROOT/volumes/ssh-hunt/secrets"
chmod 750 "$ROOT/volumes/postgres" "$ROOT/volumes/ssh-hunt" "$ROOT/volumes/backups"

if [[ -f .env.example && ! -f .env ]]; then
  cp .env.example .env
fi

if [[ ! -f "$ROOT/volumes/ssh-hunt/config.yaml" ]]; then
  cat > "$ROOT/volumes/ssh-hunt/config.yaml" <<'YAML'
server:
  listen: 0.0.0.0:22222
  rate_limit_per_second: 8
  burst: 20
ui:
  default_mode: training
  flash_default: true
redline:
  duration_seconds: 300
world:
  daily_reward_cap: 7
  style_bonus_daily_cap: 5
YAML
fi

if [[ ! -f "$ROOT/volumes/ssh-hunt/secrets/admin.yaml" ]]; then
  cat > "$ROOT/volumes/ssh-hunt/secrets/admin.yaml" <<'YAML'
# Private runtime secret. Keep this file out of version control.
username: "replace_me"
allowed_cidrs:
  - "192.168.0.0/16"
  - "10.0.0.0/8"
  - "172.16.0.0/12"
auto_keygen_on_first_login: true
required_key_fingerprint: null
YAML
  chmod 600 "$ROOT/volumes/ssh-hunt/secrets/admin.yaml"
fi

if [[ ! -f "$ROOT/volumes/ssh-hunt/secrets/hidden_ops.yaml" ]]; then
  cat > "$ROOT/volumes/ssh-hunt/secrets/hidden_ops.yaml" <<'YAML'
# Private hidden mission + optional telegram relay config.
# Keep this file private (chmod 600).
secret_mission:
  code: "replace_with_private_code"
  min_reputation: 60
  required_achievement: "Pipe Dream"
  prompt_ciphertext_b64: "AA=="
telegram:
  enabled: false
  bot_token: "replace_me"
  chat_id: "replace_me"
YAML
  chmod 600 "$ROOT/volumes/ssh-hunt/secrets/hidden_ops.yaml"
fi

echo "SSH-Hunt directories prepared under $ROOT"

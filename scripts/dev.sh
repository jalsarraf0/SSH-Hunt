#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR/ssh-hunt"

export DATABASE_URL="${DATABASE_URL:-postgres://ssh_hunt:ssh_hunt_dev@localhost:5432/ssh_hunt}"
export GAME_CONFIG_PATH="${GAME_CONFIG_PATH:-../volumes/ssh-hunt/config.yaml}"
export ADMIN_SECRET_PATH="${ADMIN_SECRET_PATH:-../volumes/ssh-hunt/secrets/admin.yaml}"
export SSH_HUNT_LISTEN="${SSH_HUNT_LISTEN:-0.0.0.0:22222}"

cargo run -p ssh_hunt_server --bin ssh-hunt-server

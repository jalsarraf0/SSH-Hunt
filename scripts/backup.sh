#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

mkdir -p ./volumes/backups
STAMP="$(date +%Y%m%d-%H%M%S)"
OUT="./volumes/backups/ssh-hunt-${STAMP}.dump"

docker compose exec -T postgres pg_dump -U "${POSTGRES_USER:-ssh_hunt}" -d "${POSTGRES_DB:-ssh_hunt}" -Fc > "$OUT"
echo "Backup written: $OUT"

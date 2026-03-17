#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

INPUT="${1:-}"
if [[ -z "$INPUT" ]]; then
  INPUT="$(ls -1 ./volumes/backups/*.dump 2>/dev/null | tail -n1 || true)"
fi

if [[ -z "$INPUT" || ! -f "$INPUT" ]]; then
  echo "No backup dump found. Pass a file path as first argument."
  exit 1
fi

docker compose exec -T postgres psql -U "${POSTGRES_USER:-ssh_hunt}" -d postgres -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname='${POSTGRES_DB:-ssh_hunt}' AND pid <> pg_backend_pid();"
docker compose exec -T postgres dropdb -U "${POSTGRES_USER:-ssh_hunt}" "${POSTGRES_DB:-ssh_hunt}" || true
docker compose exec -T postgres createdb -U "${POSTGRES_USER:-ssh_hunt}" "${POSTGRES_DB:-ssh_hunt}"
cat "$INPUT" | docker compose exec -T postgres pg_restore -U "${POSTGRES_USER:-ssh_hunt}" -d "${POSTGRES_DB:-ssh_hunt}" --clean --if-exists --no-owner --no-privileges

echo "Restore completed from $INPUT"

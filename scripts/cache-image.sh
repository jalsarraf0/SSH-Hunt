#!/usr/bin/env bash
# SSH-Hunt image cache wrapper.
# Delegates to the universal ~/scripts/cache-image.sh.
#
# Builds and caches ssh-hunt:latest in /tmp/ci-cache/ssh-hunt/
# One source of truth — no multi-version pile-up.
#
# Usage:
#   bash scripts/cache-image.sh           # build/refresh main image
#   FORGEISO_CI_FORCE=1 bash scripts/cache-image.sh  # force full rebuild

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UNIVERSAL="${HOME}/scripts/cache-image.sh"

if [[ ! -x "${UNIVERSAL}" ]]; then
    echo "ERROR: universal cache-image.sh not found at ${UNIVERSAL}" >&2
    echo "       Clone or install ~/scripts/ from the shared tooling repo." >&2
    exit 1
fi

exec "${UNIVERSAL}" \
    ssh-hunt \
    ssh-hunt:latest \
    "${REPO_ROOT}/Dockerfile" \
    "${REPO_ROOT}"

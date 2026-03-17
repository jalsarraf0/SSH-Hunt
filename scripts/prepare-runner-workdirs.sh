#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
volumes_root="${repo_root}/volumes"
runner_uid="${RUNNER_WORKDIR_UID:-1001}"
runner_gid="${RUNNER_WORKDIR_GID:-1001}"

runner_dirs=(
  "gh-runner"
  "gh-runner-ephemeral-1"
  "gh-runner-ephemeral-2"
  "gh-runner-ephemeral-3"
  "gh-runner-ephemeral-4"
)

for dir in "${runner_dirs[@]}"; do
  mkdir -p "${volumes_root}/${dir}"
done

docker run --rm \
  --user 0:0 \
  -v "${volumes_root}:/volumes" \
  alpine:3.22 \
  sh -eu -c "
for d in ${runner_dirs[*]}; do
  mkdir -p \"/volumes/\${d}\"
  chown -R ${runner_uid}:${runner_gid} \"/volumes/\${d}\"
done
"

echo "Runner workdirs prepared with ownership ${runner_uid}:${runner_gid}"

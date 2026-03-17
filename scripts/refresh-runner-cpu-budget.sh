#!/usr/bin/env bash
set -euo pipefail

env_file="${1:-.env.runner}"

if [[ ! -f "${env_file}" ]]; then
  echo "Missing ${env_file}"
  exit 1
fi

cores="$(nproc --all 2>/dev/null || nproc)"
fraction="$(grep -E '^RUNNER_TOTAL_CPU_FRACTION=' "${env_file}" | tail -n1 | cut -d= -f2- || true)"
count="$(grep -E '^RUNNER_TOTAL_COUNT=' "${env_file}" | tail -n1 | cut -d= -f2- || true)"
docker_gid="$(grep -E '^RUNNER_DOCKER_GID=' "${env_file}" | tail -n1 | cut -d= -f2- || true)"

if [[ -z "${fraction}" ]]; then
  fraction="0.75"
fi

if [[ -z "${count}" ]]; then
  count="5"
fi

if [[ -z "${docker_gid}" ]]; then
  if [[ -S /var/run/docker.sock ]]; then
    docker_gid="$(stat -c '%g' /var/run/docker.sock 2>/dev/null || true)"
  fi
fi

if [[ -z "${docker_gid}" ]]; then
  docker_gid="976"
fi

if grep -q '^RUNNER_TOTAL_CPU_FRACTION=' "${env_file}"; then
  sed -i "s/^RUNNER_TOTAL_CPU_FRACTION=.*/RUNNER_TOTAL_CPU_FRACTION=${fraction}/" "${env_file}"
else
  echo "RUNNER_TOTAL_CPU_FRACTION=${fraction}" >> "${env_file}"
fi

if grep -q '^RUNNER_TOTAL_COUNT=' "${env_file}"; then
  sed -i "s/^RUNNER_TOTAL_COUNT=.*/RUNNER_TOTAL_COUNT=${count}/" "${env_file}"
else
  echo "RUNNER_TOTAL_COUNT=${count}" >> "${env_file}"
fi

if grep -q '^RUNNER_DOCKER_GID=' "${env_file}"; then
  sed -i "s/^RUNNER_DOCKER_GID=.*/RUNNER_DOCKER_GID=${docker_gid}/" "${env_file}"
else
  echo "RUNNER_DOCKER_GID=${docker_gid}" >> "${env_file}"
fi

per_container="$(awk -v c="${cores}" -v f="${fraction}" -v n="${count}" 'BEGIN {
  if (n <= 0) n = 5;
  if (f <= 0 || f > 1) f = 0.75;
  v = (c * f) / n;
  if (v < 0.10) v = 0.10;
  printf "%.4f", v;
}')"

if grep -q '^RUNNER_CPU_LIMIT_PER_CONTAINER=' "${env_file}"; then
  sed -i "s/^RUNNER_CPU_LIMIT_PER_CONTAINER=.*/RUNNER_CPU_LIMIT_PER_CONTAINER=${per_container}/" "${env_file}"
else
  echo "RUNNER_CPU_LIMIT_PER_CONTAINER=${per_container}" >> "${env_file}"
fi

echo "Runner budget refreshed: cores=${cores}, fraction=${fraction}, count=${count}, per_container=${per_container}, docker_gid=${docker_gid}"

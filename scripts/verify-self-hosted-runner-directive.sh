#!/usr/bin/env bash
set -euo pipefail

runner_compose='docker-compose.runner.yml'
runner_dockerfile='docker/runner/Dockerfile'
runner_env_example='.env.runner.example'
runner_workdir_script='scripts/prepare-runner-workdirs.sh'

all_runs_on="$(grep -RIn --include='*.yml' 'runs-on:' .github/workflows || true)"
if [[ -z "${all_runs_on}" ]]; then
  echo "No runs-on directives found in workflows."
  exit 1
fi

violations="$(printf '%s\n' "${all_runs_on}" | grep -Ev 'vars\.SSH_HUNT_RUNNER_LABELS' || true)"
if [[ -n "${violations}" ]]; then
  echo "Runner directive violation detected. Every workflow job must use the runner selector variable:"
  echo "  runs-on: \${{ fromJSON(vars.SSH_HUNT_RUNNER_LABELS ... ) }}"
  echo ""
  echo "Violations:"
  printf '%s\n' "${violations}"
  exit 1
fi

missing_fallback="$(printf '%s\n' "${all_runs_on}" | grep -Ev '\["ubuntu-latest"\]' || true)"
if [[ -n "${missing_fallback}" ]]; then
  echo "Runner selector must keep a GitHub-hosted fallback for forks/clones."
  echo "Missing fallback in:"
  printf '%s\n' "${missing_fallback}"
  exit 1
fi

if [[ ! -f "${runner_compose}" ]]; then
  echo "Missing ${runner_compose}; self-hosted runner directive cannot be enforced."
  exit 1
fi

if [[ ! -f "${runner_dockerfile}" ]]; then
  echo "Missing ${runner_dockerfile}; custom runner image requirement not met."
  exit 1
fi

if [[ ! -f "${runner_env_example}" ]]; then
  echo "Missing ${runner_env_example}; runner CPU budget policy cannot be verified."
  exit 1
fi

if [[ ! -x "${runner_workdir_script}" ]]; then
  echo "Missing executable ${runner_workdir_script}; runner workspace permission policy cannot be verified."
  exit 1
fi

ephemeral_service_count="$(grep -En '^[[:space:]]{2}github-runner-ephemeral-[0-9]+:' "${runner_compose}" | wc -l | tr -d ' ')"
if [[ "${ephemeral_service_count}" != "4" ]]; then
  echo "Runner directive violation: ${runner_compose} must define exactly 4 ephemeral runners."
  echo "Found ${ephemeral_service_count} ephemeral services."
  exit 1
fi

for idx in 1 2 3 4; do
  if ! grep -En "^[[:space:]]{2}github-runner-ephemeral-${idx}:" "${runner_compose}" >/dev/null; then
    echo "Missing github-runner-ephemeral-${idx} in ${runner_compose}."
    exit 1
  fi
done

host_network_count="$(grep -En '^[[:space:]]{4}network_mode:[[:space:]]host$' "${runner_compose}" | wc -l | tr -d ' ')"
if [[ "${host_network_count}" != "5" ]]; then
  echo "Runner directive violation: all 5 runner services must set network_mode: host."
  echo "Found ${host_network_count} host-network declarations."
  exit 1
fi

custom_image_count="$(grep -En '^[[:space:]]{4}image:[[:space:]]ssh-hunt-gh-runner:local$' "${runner_compose}" | wc -l | tr -d ' ')"
if [[ "${custom_image_count}" != "5" ]]; then
  echo "Runner directive violation: all 5 services must use image ssh-hunt-gh-runner:local."
  echo "Found ${custom_image_count} matching image declarations."
  exit 1
fi

build_dockerfile_count="$(grep -En '^[[:space:]]{6}dockerfile:[[:space:]]docker/runner/Dockerfile$' "${runner_compose}" | wc -l | tr -d ' ')"
if [[ "${build_dockerfile_count}" != "5" ]]; then
  echo "Runner directive violation: all 5 services must build from docker/runner/Dockerfile."
  echo "Found ${build_dockerfile_count} matching dockerfile declarations."
  exit 1
fi

cpu_limit_count="$(grep -En '^[[:space:]]{4}cpus:[[:space:]]\$\{RUNNER_CPU_LIMIT_PER_CONTAINER:-0\.1500\}$' "${runner_compose}" | wc -l | tr -d ' ')"
if [[ "${cpu_limit_count}" != "5" ]]; then
  echo "Runner directive violation: all 5 services must enforce RUNNER_CPU_LIMIT_PER_CONTAINER."
  echo "Found ${cpu_limit_count} CPU limit declarations."
  exit 1
fi

run_as_root_false_count="$(grep -En '^[[:space:]]{6}RUN_AS_ROOT:[[:space:]]"false"$' "${runner_compose}" | wc -l | tr -d ' ')"
if [[ "${run_as_root_false_count}" != "5" ]]; then
  echo "Runner security directive violation: all 5 services must set RUN_AS_ROOT=false."
  echo "Found ${run_as_root_false_count} RUN_AS_ROOT=false declarations."
  exit 1
fi

docker_gid_map_count="$(grep -En '^[[:space:]]{6}-[[:space:]]"\$\{RUNNER_DOCKER_GID:-976\}"$' "${runner_compose}" | wc -l | tr -d ' ')"
if [[ "${docker_gid_map_count}" != "5" ]]; then
  echo "Runner security directive violation: all 5 services must map RUNNER_DOCKER_GID in group_add."
  echo "Found ${docker_gid_map_count} RUNNER_DOCKER_GID mappings."
  exit 1
fi

if ! grep -Eq '^RUNNER_TOTAL_CPU_FRACTION=0\.75$' "${runner_env_example}"; then
  echo "Runner CPU policy violation: ${runner_env_example} must define RUNNER_TOTAL_CPU_FRACTION=0.75."
  exit 1
fi

if ! grep -Eq '^RUNNER_TOTAL_COUNT=5$' "${runner_env_example}"; then
  echo "Runner CPU policy violation: ${runner_env_example} must define RUNNER_TOTAL_COUNT=5."
  exit 1
fi

if ! grep -Eq '^RUNNER_CPU_LIMIT_PER_CONTAINER=' "${runner_env_example}"; then
  echo "Runner CPU policy violation: ${runner_env_example} must define RUNNER_CPU_LIMIT_PER_CONTAINER."
  exit 1
fi

if ! grep -Eq '^RUNNER_DOCKER_GID=' "${runner_env_example}"; then
  echo "Runner socket policy violation: ${runner_env_example} must define RUNNER_DOCKER_GID."
  exit 1
fi

if ! grep -En '^USER runner$' "${runner_dockerfile}" >/dev/null; then
  echo "Runner security directive violation: ${runner_dockerfile} must end with USER runner."
  exit 1
fi

if ! grep -En 'trivy_' "${runner_dockerfile}" >/dev/null; then
  echo "Runner tooling directive violation: ${runner_dockerfile} must install trivy."
  exit 1
fi

if ! grep -En '^runner-up: runner-env' Makefile >/dev/null; then
  echo "Runner directive violation: Makefile runner-up target is missing."
  exit 1
fi

if ! grep -En 'runner-workdirs' Makefile >/dev/null; then
  echo "Runner directive violation: Makefile must include runner-workdirs preparation."
  exit 1
fi

if ! grep -En 'RUNNER_COMPOSE := docker compose --env-file \.env\.runner -f docker-compose\.runner\.yml' Makefile >/dev/null; then
  echo "Runner directive violation: RUNNER_COMPOSE must use --env-file .env.runner."
  exit 1
fi

if ! grep -En '\$\(RUNNER_COMPOSE\) up -d --build' Makefile >/dev/null; then
  echo "Runner directive violation: runner-up must bring up docker-compose.runner.yml services."
  exit 1
fi

if ! grep -En '\$\(MAKE\) --no-print-directory runner-workdirs' Makefile >/dev/null; then
  echo "Runner directive violation: runner-up must normalize runner workspace ownership."
  exit 1
fi

if ! grep -En 'refresh-runner-cpu-budget\.sh \.env\.runner' Makefile >/dev/null; then
  echo "Runner directive violation: runner CPU budget refresh step missing from Makefile."
  exit 1
fi

echo "Runner selector and ephemeral pool directive verified across workflows."

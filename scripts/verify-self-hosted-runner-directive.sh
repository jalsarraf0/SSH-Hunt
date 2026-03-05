#!/usr/bin/env bash
set -euo pipefail

runner_compose='docker-compose.runner.yml'
runner_dockerfile='docker/runner/Dockerfile'
runner_env_example='.env.runner.example'

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

if ! grep -En '^runner-up: runner-env' Makefile >/dev/null; then
  echo "Runner directive violation: Makefile runner-up target is missing."
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

if ! grep -En 'refresh-runner-cpu-budget\.sh \.env\.runner' Makefile >/dev/null; then
  echo "Runner directive violation: runner CPU budget refresh step missing from Makefile."
  exit 1
fi

echo "Runner selector and ephemeral pool directive verified across workflows."

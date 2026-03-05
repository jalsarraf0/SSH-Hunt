#!/usr/bin/env bash
set -euo pipefail

workflow_glob='.github/workflows/*.yml'

all_runs_on="$(rg -n 'runs-on:' ${workflow_glob} || true)"
if [[ -z "${all_runs_on}" ]]; then
  echo "No runs-on directives found in workflows."
  exit 1
fi

violations="$(printf '%s\n' "${all_runs_on}" | rg -v 'vars\.SSH_HUNT_RUNNER_LABELS' || true)"
if [[ -n "${violations}" ]]; then
  echo "Runner directive violation detected. Every workflow job must use the runner selector variable:"
  echo "  runs-on: \${{ fromJSON(vars.SSH_HUNT_RUNNER_LABELS ... ) }}"
  echo ""
  echo "Violations:"
  printf '%s\n' "${violations}"
  exit 1
fi

missing_fallback="$(printf '%s\n' "${all_runs_on}" | rg -v '\["ubuntu-latest"\]' || true)"
if [[ -n "${missing_fallback}" ]]; then
  echo "Runner selector must keep a GitHub-hosted fallback for forks/clones."
  echo "Missing fallback in:"
  printf '%s\n' "${missing_fallback}"
  exit 1
fi

echo "Runner selector directive verified across workflows."

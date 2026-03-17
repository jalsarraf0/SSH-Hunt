# Self-Hosted GitHub Runner (Docker)

This repo supports running GitHub Actions on your own server using a Dockerized runner.

## 1) Prepare Runner Env

```bash
cp .env.runner.example .env.runner
```

Set:

- `ACCESS_TOKEN`: Personal Access Token with `repo` and `workflow` scopes.
- Optional: `RUNNER_NAME`, `RUNNER_NAME_PREFIX`, `LABELS`, `RUNNER_EPHEMERAL`, `RUNNER_DOCKER_GID`.

## 2) Start Runner Containers

```bash
make runner-up
make runner-ps
make runner-logs
```

`make runner-up` uses `.env.runner` as the compose env file, so CPU budget settings are applied at compose render time.
It also normalizes ownership on `volumes/gh-runner*` before start to prevent stale root-owned action cache files from breaking non-root runners.

Default stack:

- `1` persistent runner (`github-runner`)
- `4` ephemeral burst runners (`github-runner-ephemeral-1..4`)
- all runners use `network_mode: host` so GitHub Actions service-container ports (for example Postgres in CI) are reachable from runner jobs.
- all runners are built from `docker/runner/Dockerfile`, which preinstalls core CI/CD tools (`ripgrep`, `jq`, `gitleaks`, `osv-scanner`, `trivy`, and downloader utilities).
- CPU budget directive: total runner pool is capped at `75%` of host cores, split across all 5 containers by `scripts/refresh-runner-cpu-budget.sh`.
- runner stack defaults to non-root execution (`RUN_AS_ROOT=false`) and maps the host Docker socket group via `RUNNER_DOCKER_GID`.

Runner labels (all runners) default to:

- `self-hosted`
- `linux`
- `x64`
- `ssh-hunt`

## 3) Enable Self-Hosted Execution For This Repo

Workflows use a selector variable:

- `SSH_HUNT_RUNNER_LABELS` (JSON array string)

Set for this repository:

```bash
gh variable set SSH_HUNT_RUNNER_LABELS --body '["self-hosted","linux","x64","ssh-hunt"]'
```

For GitHub-hosted fallback:

```bash
gh variable set SSH_HUNT_RUNNER_LABELS --body '["ubuntu-latest"]'
```

If the variable is absent, workflows default to GitHub-hosted `ubuntu-latest` to keep forks/clones usable.

## 4) Directive Enforcement

Runner selection policy is checked by:

```bash
./scripts/verify-self-hosted-runner-directive.sh
```

Security workflow runs this check so policy drift is caught automatically.
It also enforces that `docker-compose.runner.yml` keeps all four ephemeral runner services.

## Security Notes

- Self-hosted runners execute arbitrary workflow code. Treat them as trusted-internal infrastructure.
- Do not run untrusted fork workflows on privileged runners.
- Docker socket is mounted for build/publish jobs; this is intentionally privileged.

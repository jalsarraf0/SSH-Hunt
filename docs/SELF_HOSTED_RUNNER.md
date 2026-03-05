# Self-Hosted GitHub Runner (Docker)

This repo supports running GitHub Actions on your own server using a Dockerized runner.

## 1) Prepare Runner Env

```bash
cp .env.runner.example .env.runner
```

Set:

- `ACCESS_TOKEN`: Personal Access Token with `repo` and `workflow` scopes.
- Optional: `RUNNER_NAME`, `LABELS`, `RUNNER_EPHEMERAL`.

## 2) Start Runner Container

```bash
make runner-up
make runner-ps
make runner-logs
```

Runner labels default to:

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

## Security Notes

- Self-hosted runners execute arbitrary workflow code. Treat them as trusted-internal infrastructure.
- Do not run untrusted fork workflows on privileged runners.
- Docker socket is mounted for build/publish jobs; this is intentionally privileged.

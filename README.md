# SSH-Hunt

[![CI](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/ci.yml/badge.svg)](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/ci.yml)

> CI runs on self-hosted runners managed by [haskell-ci-orchestrator](https://github.com/jalsarraf0/haskell-ci-orchestrator) with build attestation. Lint, test, security, SBOM, Docker, and release jobs are unified in a single pipeline.

SSH-Hunt is a publicly playable cyberpunk SSH game and terminal learning MMO.

- Fully simulated shell and world, implemented in Rust.
- Teaches practical shell habits through missions and progression.
- Includes Training Sim, NetCity multiplayer, REDLINE timed runs, scripts, auction, and PvP.

Live public connection endpoint:

`ssh -p 24444 <username>@ssh-hunt.appnest.cc`

## Table of Contents

- [What This Is](#what-this-is)
- [Security Guarantees](#security-guarantees)
- [Quick Start](#quick-start)
- [Connect and First Login](#connect-and-first-login)
- [How to Play](#how-to-play)
- [Command Reference](#command-reference)
- [Cloudflare Tunnel Target (Exact)](#cloudflare-tunnel-target-exact)
- [Deployment and Ops](#deployment-and-ops)
- [Configuration and Secrets](#configuration-and-secrets)
- [CI/CD and Security Pipeline](#cicd-and-security-pipeline)
- [Troubleshooting](#troubleshooting)
- [Contributing](#contributing)
- [License](#license)

## What This Is

SSH-Hunt is designed for hostile internet exposure while keeping gameplay isolated from the host OS.
Players connect with a normal SSH client and interact with a simulated terminal, missions, economy, and multiplayer systems.

Interactive shell prompt format:

`<ssh_username>@<node>:/path$`

Player identity shown in social/leaderboard views:

`<ssh_username>@<remote_ip>`

## Security Guarantees

SSH-Hunt does **not** execute real host shell commands.
Gameplay command behavior is implemented against:

- virtual filesystem (VFS),
- simulated world state and mission engine,
- sandboxed scripting engine,
- PostgreSQL persistence.

Hard defensive guarantees:

- no `std::process::Command` or `tokio::process::Command` in game server paths,
- no host filesystem gameplay access outside mounted runtime data,
- no host service control or breakout command path,
- breakout/probing attempts trigger immediate permanent zero + disconnect.

## Quick Start

1. Install Docker and Docker Compose.
2. Clone to `/docker/ssh-hunt` (or adjust paths below to your location).
3. Initialize runtime files and start services:

```bash
cd /docker/ssh-hunt
./scripts/install.sh
cp .env.example .env
make up
```

4. Verify:

```bash
make ps
make logs
```

Default published SSH port is `24444` on the host.

## Connect and First Login

Connect from any SSH client:

```bash
ssh -p 24444 <username>@<server-or-hostname>
```

Live server:

```bash
ssh -p 24444 <username>@ssh-hunt.appnest.cc
```

Password is not required by default for gameplay login flow.

## How to Play

### 1) Start Tutorial and Mission Flow

After login, run:

```text
tutorial start
guide
tutorial start
missions
accept keys-vault
```

### 2) Complete KEYS VAULT (Required)

Generate a client key locally:

```bash
ssh-keygen -t ed25519 -a 64 -f ~/.ssh/ssh-hunt_ed25519
```

Then in-game:

```text
keyvault register
```

Paste your public key line when prompted.

### 3) Unlock NetCity

NetCity unlock requirements:

- complete `keys-vault`,
- complete at least one starter mission (`pipes-101`, `finder`, `redirect-lab`, `log-hunt`, `dedupe-city`),
- log in presenting your registered key.

Then switch:

```text
mode netcity
```

### 4) REDLINE Timed Mode

```text
mode redline
settings flash off
```

REDLINE auto-returns to Training when timer expires.

### 5) Economy, Scripts, PvP, Rankings

```text
status
gate
events
auction list
scripts market
scripts run token-hunt
leaderboard
tier noob|gud|hardcore
pvp roster
pvp challenge <username>
pvp attack
pvp defend
pvp script <script_name>
```

`Hardcore` players are permanently zeroed after 3 deaths.

### 6) Social Commands

```text
chat global <message>
chat sector <message>
chat party <message>
mail inbox
party invite <username>
```

### 7) Defense Policy (Player-Facing)

Any attempt to break out into host runtime/tools is treated as intrusion:

- account permanently zeroed,
- session disconnected immediately.

## Command Reference

Core:

- `help`
- `guide [quick|full]`
- `tutorial start`
- `missions`
- `accept <mission-code>`
- `submit <mission-code>`
- `mode <training|netcity|redline>`
- `gate`
- `settings flash <on|off>`
- `keyvault register [ssh-public-key-line]`
- `status`
- `events`
- `leaderboard [N]`
- `daily`
- `tier <noob|gud|hardcore>`

Economy:

- `inventory`
- `shop list`
- `shop buy <sku>`
- `auction list`
- `auction sell <sku> <qty> <start_price> [buyout]`
- `auction bid <listing_id> <amount>`
- `auction buyout <listing_id>`

PvP:

- `pvp roster`
- `pvp challenge <username>`
- `pvp attack`
- `pvp defend`
- `pvp script <script_name>`

Scripts:

- `scripts market`
- `scripts run <name>`

## Cloudflare Tunnel Target (Exact)

For the current Docker setup in this repo, point the tunnel origin to:

`ssh://localhost:24444`

Why:

- container listens on `22222`,
- compose publishes `${SSH_HUNT_PORT:-24444}:22222`,
- so host-side tunnel should target host port `24444`.

If you run `cloudflared` inside the same Docker network as `ssh-hunt`, target:

`ssh://ssh-hunt:22222`

If you are **not** using Cloudflare Tunnel and are exposing `ssh-hunt.appnest.cc` with an A/AAAA record:

- keep DNS record as **DNS only** (no HTTP proxy),
- proxy mode can break raw SSH on custom port `24444`.

## Deployment and Ops

Primary operator commands:

```bash
make up
make down
make ps
make logs
make restart
make doctor
make firewall-open-24444
make firewall-status
make db-migrate
make db-seed
make test
make backup
make restore
```

Fedora firewalld example:

```bash
sudo firewall-cmd --permanent --add-port=24444/tcp
sudo firewall-cmd --reload
```

See full guide: `docs/DEPLOYMENT.md`

## Configuration and Secrets

Environment template: `.env.example`

Important defaults:

- `SSH_HUNT_PORT=24444`
- `SSH_HUNT_LISTEN=0.0.0.0:22222`
- `DATABASE_URL=postgres://ssh_hunt:ssh_hunt_dev@postgres:5432/ssh_hunt`
- `HIDDEN_OPS_PATH=/data/secrets/hidden_ops.yaml`
- `SSH_HOST_KEY_PATH=/data/secrets/ssh_host_ed25519`

Runtime-only secret files (not committed):

- `/docker/ssh-hunt/volumes/ssh-hunt/secrets/admin.yaml`
- `/docker/ssh-hunt/volumes/ssh-hunt/secrets/hidden_ops.yaml`
- `/docker/ssh-hunt/volumes/ssh-hunt/secrets/ssh_host_ed25519` (persistent SSH host key)

Keep runtime secret files private (`chmod 600`).
If `SSH_HOST_KEY_PATH` is not writable at runtime, server falls back to an ephemeral key for that process and logs a warning.

## CI/CD and Security Pipeline

Pipelines include:

- workspace build checks and formatting,
- clippy with warnings denied,
- full unit/integration/regression test suite,
- SQL migration checks,
- `cargo audit` and `cargo deny`,
- secret scanning (`gitleaks`),
- static analysis (`CodeQL`),
- vulnerability scans (`trivy`, `osv-scanner`),
- Docker build/push, signed image workflow, SBOM/provenance.

Runner policy:

- workflows use `SSH_HUNT_RUNNER_LABELS` (JSON array string) to choose runner labels,
- default fallback is GitHub-hosted `["ubuntu-latest"]` for forks/clones,
- this repo can be forced to self-hosted by setting:
  `gh variable set SSH_HUNT_RUNNER_LABELS --body '["self-hosted","linux","x64","ssh-hunt"]'`,
- self-hosted compose stack must include `1` persistent + `4` ephemeral runners,
- self-hosted runners use host networking so CI service containers are reachable,
- runner CPU policy: total runner pool budget is `75%` of host cores, auto-divided across 5 containers via `scripts/refresh-runner-cpu-budget.sh`,
- runner containers default to non-root mode and map host docker socket group (`RUNNER_DOCKER_GID`) for Docker build access,
- policy enforcement script: `./scripts/verify-self-hosted-runner-directive.sh` runs on every `push` in Security workflow.

Self-hosted runner setup:

- `cp .env.runner.example .env.runner`
- `make runner-up` (refreshes CPU budget, normalizes runner workdir ownership, builds `docker/runner/Dockerfile`, then starts persistent + 4 ephemeral runners using `.env.runner`)
- `make runner-logs`
- full guide: `docs/SELF_HOSTED_RUNNER.md`

## Troubleshooting

Service not reachable:

- confirm `make ps` shows healthy containers,
- verify host port with `ss -ltnp | rg 24444`,
- check game logs with `make logs`.
- run `make doctor` for a one-shot local health summary.
- run `make firewall-open-24444` to open `24444/tcp` in all firewalld zones.
- run `make firewall-status` to verify active zone + `24444/tcp` allow state.

`Connection refused` specifically usually means no listener at that moment.
Most common causes:

- containers are stopped,
- compose startup failed (for example missing `.env`),
- host listener exists but external NAT/port-forward path is disabled.

Windows `ssh -vvvv` notes:

- `Failed to open .../.ssh/config error:2` is non-fatal when those files do not exist.
- `error: 10061` means TCP was actively refused at the target edge (service down or wrong port-forward destination).
- `error: 10060` means timeout (traffic dropped/blocked on path).

Cross-shell UI compatibility notes (PowerShell, bash, zsh, macOS Terminal, iTerm2):

- server normalizes all output to CRLF over SSH PTY so banners/prompt do not stair-step,
- CR, LF, and CRLF Enter key variants are de-duplicated server-side,
- terminal escape input sequences (for example arrow keys) are ignored safely in command buffer,
- very narrow terminals use compact headers/sections to avoid broken wrapping,
- frame rendering auto-falls back to ASCII on low-capability terminals and adapts on terminal resize.

If you see host key warnings after server rebuild:

- SSH-Hunt now persists host key by default at `/data/secrets/ssh_host_ed25519`.
- warning should only happen on first deploy or intentional key rotation.

Public hostname checklist for `ssh-hunt.appnest.cc`:

- test local host listener: `ss -ltnp | rg 24444`,
- confirm containers: `make ps`,
- confirm firewalld port: `sudo firewall-cmd --zone=lan-ssh --query-port=24444/tcp`,
- confirm router/NAT rule: WAN `TCP/24444` -> `<server-lan-ip>:24444`,
- confirm DNS currently resolves to your active public IP (for example on March 5, 2026 this host resolves to `70.233.5.234`).

NetCity still locked:

- verify `keys-vault` is completed,
- complete one starter mission,
- reconnect with the same registered key loaded in SSH client.

Database issues:

- run `make db-migrate`,
- run `make db-seed`,
- inspect postgres logs via `docker compose logs postgres`.

## Contributing

- Read `docs/GAMEPLAY.md`, `docs/SECURITY.md`, `docs/DEPLOYMENT.md`, and `docs/SELF_HOSTED_RUNNER.md`.
- Follow `CODE_OF_CONDUCT.md`.
- Run `make test` before opening a PR.

## License

Dual-licensed under:

- MIT (`LICENSE-MIT`)
- Apache-2.0 (`LICENSE-APACHE`)

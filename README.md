# SSH-Hunt

[![CI](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/ci.yml/badge.svg)](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/ci.yml)
[![Security](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/security.yml/badge.svg)](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/security.yml)
[![CodeQL](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/codeql.yml/badge.svg)](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/codeql.yml)
[![Deep Sweep](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/deep-security-sweep.yml/badge.svg)](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/deep-security-sweep.yml)
[![Docker](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/docker.yml/badge.svg)](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/docker.yml)

SSH-Hunt is a publicly playable cyberpunk terminal MMO/learning game over SSH.

- 100% simulated shell and world (no host command execution).
- Teaches real bash habits through missions and story.
- Training Sim (green) and NetCity MMO (purple neon) with REDLINE timed runs.

## Security Model

SSH-Hunt does **not** execute real OS shell commands.
All command behavior is implemented in Rust against:

- virtual filesystem (VFS),
- simulated world state,
- player economy/mission systems,
- PostgreSQL persistence.

Hard blocks:

- no `std::process::Command` in game server code,
- no real network scanning/probing,
- no host filesystem access except `/data` and Postgres connectivity.
- breakout/probing attempts trigger immediate permanent zero + disconnect.

## Quickstart

1. Install Docker + Docker Compose.
2. Clone into `/docker/ssh-hunt`.
3. Initialize runtime directories and local env.

```bash
cd /docker/ssh-hunt
./scripts/install.sh
cp .env.example .env
make up
```

4. Connect with any SSH client:

```bash
ssh -p 24444 snake@your.domain
```

Display identity is always `<ssh_username>@<remote_ip>`.

## How To Expose Safely

Only one TCP port is published (default `24444`).

Fedora firewalld example:

```bash
sudo firewall-cmd --permanent --add-port=24444/tcp
sudo firewall-cmd --reload
```

Recommended host protections:

- CrowdSec or Fail2ban for SSH flood/bruteforce pressure,
- reverse-path filtering and basic DDoS mitigations,
- keep host patched,
- monitor logs and rate-limit alerts.

## How To Play

1. Login via SSH (no password required).
2. Start onboarding:

```text
tutorial start
missions
accept keys-vault
```

3. Complete **KEYS VAULT** mission:

```bash
ssh-keygen -t ed25519 -a 64 -f ~/.ssh/ssh-hunt_ed25519
```

Then paste your public key line in-game via `keyvault register`.

4. Complete one additional starter mission.
5. Unlock NetCity:

```text
mode netcity
```

6. REDLINE mode:

```text
mode redline
settings flash off
```

7. PvP and difficulty tiers:

```text
tier noob|gud|hardcore
pvp roster
pvp challenge <username>
pvp attack
pvp defend
pvp script <script_name>
```

`Hardcore` players are zeroed (locked) after 3 deaths.

8. Automation and world feeds:

```text
status
events
auction list
scripts market
scripts run token-hunt
```

Any attempt to escape into host runtime/tools results in immediate permanent ban and disconnect.

## Ops Commands

```bash
make up
make logs
make db-migrate
make db-seed
make test
make backup
make restore
```

Private hidden mission and Telegram relay settings are runtime-only under
`/docker/ssh-hunt/volumes/ssh-hunt/secrets/hidden_ops.yaml` and are never committed.

## CI/CD and Security Evidence

Pipelines include:

- full regression tests (unit + integration + SSH flow tests),
- sqlx migration checks,
- cargo audit / cargo deny / dependency review,
- secret scanning,
- CodeQL static analysis,
- container and dependency vulnerability scanning,
- signed GHCR images, SBOM, and provenance attestations,
- weekly deep security sweep + automated dependency update PRs.

## Contributing

- Read `docs/GAMEPLAY.md`, `docs/SECURITY.md`, and `docs/DEPLOYMENT.md`.
- Follow the Code of Conduct.
- Run `make test` before submitting a PR.

## License

Dual-licensed under:

- MIT (`LICENSE-MIT`)
- Apache-2.0 (`LICENSE-APACHE`)

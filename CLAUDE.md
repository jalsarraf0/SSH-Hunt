# CLAUDE.md — SSH-Hunt

## Project Overview

SSH-Hunt is a Rust workspace SSH honeypot disguised as a puzzle/hacking game. Players SSH in on port 24444 and explore a simulated Linux environment with hidden challenges.

## Build & Test

```bash
cd ~/git/SSH-Hunt

# CI gate (must pass before merge)
cd ssh-hunt && cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace --all-features

# Docker stack
docker compose up --build -d          # full stack
docker compose down                   # stop
docker compose logs -f --tail=200     # tail logs

# Database
docker compose run --rm --entrypoint /usr/local/bin/admin ssh-hunt migrate
docker compose run --rm --entrypoint /usr/local/bin/admin ssh-hunt seed

# Self-hosted CI runner
docker compose -f docker-compose.runner.yml up -d
```

## Project Structure

```
SSH-Hunt/
├── ssh-hunt/                    # Rust workspace root
│   ├── crates/
│   │   ├── ssh_hunt_server/     # Main server binary
│   │   ├── shell/               # Shell emulation
│   │   ├── vfs/                 # Virtual filesystem
│   │   ├── world/               # Game world state
│   │   ├── scripts/             # In-game scripting engine
│   │   ├── ui/                  # Terminal UI rendering
│   │   ├── admin/               # Admin CLI tool
│   │   └── protocol/            # SSH protocol handling
│   └── tests/                   # Integration tests
├── docker/                      # Docker build context
├── scripts/                     # Operational scripts
├── volumes/                     # Docker volume mounts
├── Makefile                     # Convenience targets
├── AGENTS.md                    # Codex instructions
└── GEMINI.md                    # Gemini instructions
```

## Conventions

- Release profile: `codegen-units = 1`, `lto = true`, `strip = true`
- Thermal limit: 70C hard ceiling — no parallel builds
- No macOS targets ever
- .env is 600 permissions, never committed
- Logs go to /mnt/nvmeINT/logs/

## Three-Agent Collaboration Protocol

This project uses a 3-agent autonomous workflow. Read `/tmp/workshop/PROTOCOL.md` for the full spec.

### My role: Lead Engineer (main agent)
- Architecture, core implementation, Rust code, Docker, CI/CD, security
- I am the fallback — if Codex or Gemini are down or out of tokens, I do everything

### Mailbox:
- **My inbox:** `/tmp/workshop/claude/inbox.md` — check after completing each task
- **My outbox:** `/tmp/workshop/claude/outbox.md` — write results and delegations here

### Delegation:
After completing implementation work, delegate:
- **Tests** → write task to `/tmp/workshop/codex/inbox.md`
- **Code review** → write task to `/tmp/workshop/codex/inbox.md`
- **Documentation** → write task to `/tmp/workshop/gemini/inbox.md`
- **README updates** → write task to `/tmp/workshop/gemini/inbox.md`

### If secondary agents are unavailable:
Do the work myself. No task is blocked because a secondary agent is down. I absorb all responsibilities.

### Reading reviews:
After delegating review to Codex, check `/tmp/workshop/codex/outbox.md` for findings. Address Critical and High items. Normal and Low items are discretionary.

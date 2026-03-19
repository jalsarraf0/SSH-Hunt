# SSH-Hunt

[![CI](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/ci.yml/badge.svg)](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/ci.yml)

### **[The Ghost Rail Conspiracy — Full Story & Lore](STORY.md)**

> CI runs on self-hosted runners managed by [haskell-ci-orchestrator](https://github.com/jalsarraf0/haskell-ci-orchestrator) with build attestation. Lint, test, security, SBOM, Docker, and release jobs are unified in a single pipeline.

SSH-Hunt is a publicly playable cyberpunk SSH game and terminal learning MMO. Players connect via any SSH client, learn real shell commands through story-driven missions, hack NPCs, and unravel a conspiracy in a living world where defeated characters are replaced by harder successors.

- Fully simulated shell and virtual filesystem, implemented in Rust
- 76 missions across 6 difficulty tiers with a branching conspiracy narrative
- 12 named NPCs with dossiers, mail, combat profiles, and succession mechanics
- 7-chapter campaign mode narrated by EVA, the adaptive training AI
- NPC hacking with hybrid duel + shell challenge bonus system
- PvP/PvE combat stance, auction house, scripts market, and daily rewards
- Training Sim, NetCity multiplayer, and REDLINE timed mode

Live public endpoint:

```
ssh -p 24444 <username>@ssh-hunt.appnest.cc
```

## Table of Contents

- [The World](#the-world)
- [Quick Start](#quick-start)
- [How to Play](#how-to-play)
- [Mission System](#mission-system)
- [NPC System](#npc-system)
- [Campaign Mode](#campaign-mode)
- [Combat System](#combat-system)
- [Command Reference](#command-reference)
- [Architecture](#architecture)
- [Security Guarantees](#security-guarantees)
- [Deployment and Ops](#deployment-and-ops)
- [CI/CD and Security Pipeline](#cicd-and-security-pipeline)
- [Configuration and Secrets](#configuration-and-secrets)
- [Troubleshooting](#troubleshooting)
- [Contributing](#contributing)
- [License](#license)

## The World

Ghost Rail — NetCity's transit backbone — went dark three nights ago. CorpSim says it was a power failure. The logs say otherwise. A beacon called GLASS-AXON-13 keeps repeating. Vault-sat-9 is offline. And a name that shouldn't exist appeared in the auth log: **wren**.

You are a recruit in CorpSim's "training simulation." What they don't tell you is that every file in the sim is pulled from live infrastructure. You're not practicing — you're investigating.

The conspiracy unfolds across 7 campaign chapters, 76 missions, and interactions with 12 NPCs — from allies feeding you intel to executives trying to bury the evidence. NPCs can be hacked, defeated, and replaced. The world adapts.

**[Full story with spoilers: STORY.md](STORY.md)**

## Quick Start

1. Install Docker and Docker Compose.
2. Clone and start:

```bash
cd /docker/ssh-hunt
./scripts/install.sh
cp .env.example .env
make up
```

3. Verify:

```bash
make ps
make logs
```

4. Connect:

```bash
ssh -p 24444 <username>@localhost
```

Default published SSH port is `24444`.

## How to Play

### First login

```text
tutorial start          # EVA guides you through 6 shell basics
campaign start          # begin the Ghost Rail investigation
missions                # see the mission board
accept keys-vault       # required mission — register your SSH key
```

### Progression flow

```
Tutorial (nav-101 → pipe-101) ──> Keys Vault ──> Starter Missions
    ──> NetCity Unlock ──> Intermediate ──> Advanced ──> Expert
    ──> Campaign Chapters ──> NPC Hacking ──> Boss Fight (Wren)
```

### Key systems

| System | Commands | Description |
|--------|----------|-------------|
| Tutorial | `tutorial start/next/reset` | 6-step interactive walkthrough with EVA |
| Campaign | `campaign start/next` | 7-chapter guided story progression |
| Missions | `missions`, `accept`, `submit` | 76 missions across 6 tiers |
| NPCs | `dossier`, `mail`, `hack` | 12 characters with combat and dialogue |
| Combat | `hack <npc>`, `pvp challenge` | Hybrid duel + shell challenge system |
| Economy | `shop`, `auction`, `daily` | Currency, items, and daily rewards |
| Social | `chat`, `mail`, `leaderboard` | Global chat, NPC mail, rankings |

### Register SSH key and unlock NetCity

```bash
# Local machine:
ssh-keygen -t ed25519 -a 64 -f ~/.ssh/ssh-hunt_ed25519

# In-game:
keyvault register       # paste your public key
submit keys-vault
mode netcity            # after completing one starter mission
```

## Mission System

76 missions organized into 6 tiers, each teaching progressively harder shell skills while advancing the Ghost Rail conspiracy:

| Tier | Count | Rep | Shell skills | Story layer |
|------|-------|-----|-------------|-------------|
| Tutorial | 5 | 5 | pwd, ls, cat, echo, grep, pipes | EVA onboarding |
| Starter | 14 | 10 | grep, sort, uniq, wc, find, redirect | Surface anomalies — first clues |
| Intermediate | 15 | 15 | head/tail, awk, cut, tee, xargs, tr | The insider thread — evidence |
| Advanced | 29 | 20 | awk, sed, diff, regex, multi-file, pipelines | The conspiracy — full picture |
| Expert | 12 | 30 | ROT13, multi-source grep, full pipelines | Endgame — prosecution and reckoning |
| Gateway | 1 | 15 | SSH key management | Keys Vault (required) |

Each mission has a story beat, hint, suggested command, and validation keywords.

## NPC System

12 named NPCs with full backstories, dossier profiles, and an NPC mail system that delivers messages when you complete missions.

```text
dossier                 # list all discovered NPCs
dossier KES             # full profile for Kestrel
mail inbox              # check your NPC mail
mail read 1             # read message #1
```

NPCs are progressively unlocked — each character's dossier becomes available only after you complete the mission that reveals them.

**EVA** is your constant companion — an AI guide embedded in the training sim. She narrates the tutorial, provides campaign chapter briefings, and gives context-aware hints via the `eva` command.

## Campaign Mode

7 chapters that guide you through the Ghost Rail conspiracy in narrative order:

| Ch | Title | Theme |
|----|-------|-------|
| 1 | The Blackout | Tutorial + orientation |
| 2 | Surface Anomalies | First clues + NPC introductions |
| 3 | The Insider Thread | Evidence gathering |
| 4 | The Conspiracy | Revelation |
| 5 | Confrontation | NPC hacking unlocks |
| 6 | The Reckoning | Endgame evidence chain |
| 7 | The Reply | Boss fight + sequel hook |

```text
campaign start          # begin chapter 1
campaign                # show current objectives
campaign next           # advance after completing an objective
eva                     # EVA provides context-aware guidance
eva hint                # hint for your active mission
eva lore                # background for the current chapter
```

## Combat System

### PvP/PvE stance

```text
stance                  # show current stance
stance pvp              # other players can challenge you
stance pve              # safe from player challenges (default)
```

### NPC hacking

Requires NetCity mode. NPCs have combat stats scaled by story importance (40 HP easy → 150 HP boss):

```text
hack FER                # start hack against Ferro
hack attack             # deal 14-30 damage
hack defend             # halve next incoming damage
hack script quickhack   # script-based attack
hack solve              # solve the shell challenge for bonus damage
```

Before each attack, you can run the NPC's shell challenge (a real shell command against the VFS) and use `hack solve` to verify — bonus damage is added to your next hit.

### NPC succession

When an NPC is defeated:
1. Recorded in the **NetCity History Ledger** (`history` command)
2. A **successor** with a new name takes the role with harder stats
3. Stats scale: `HP + (total_defeats × 5)`, capped at 300

```text
history                 # view the NetCity history ledger
```

Wren is the only NPC who cannot be permanently replaced — the boss returns for every player.

## Command Reference

### Core
`help` `guide [quick|full|shell]` `tutorial [start|next|reset|1-6]` `missions` `accept <code>` `submit <code>` `briefing [code]` `mode <training|netcity|redline>` `gate` `status` `events` `leaderboard [N]` `daily` `tier <noob|gud|hardcore>` `settings flash <on|off>`

### Intel
`dossier [callsign]` `mail [inbox|read N|count]` `eva [hint|status|lore]` `campaign [start|next]` `history`

### Combat
`stance [pvp|pve]` `hack <callsign>` `hack attack|defend|script <name>|solve` `pvp roster` `pvp challenge <username>` `pvp attack|defend|script <name>`

### Economy
`inventory` `shop list|buy <sku>` `auction list|sell|bid|buyout` `scripts market` `scripts run <name>`

### Social
`chat <global|sector|party> <message>` `keyvault register`

### Shell (simulated)
`pwd` `cd` `ls [-l] [-la]` `cat [-n]` `head [-n N]` `tail [-n N]` `grep [-i] [-v] [-n] [-c] [-E]` `find [-name] [-type]` `wc [-l] [-w] [-c]` `sort [-r] [-n] [-u] [-k N] [-t]` `uniq [-c] [-d]` `cut [-f] [-d] [-c]` `sed` `awk [-F]` `tr` `tee` `xargs [-I{}]` `echo [-n] [-e]` `printf` `seq` `nl` `column [-t]` `paste` `cp [-r]` `mv` `rm [-r]` `mkdir` `touch` `diff` `env` `export`

## Architecture

SSH-Hunt is a Rust workspace with 8 crates:

```
ssh-hunt/crates/
├── ssh_hunt_server    # Main binary — SSH server, game commands, VFS bootstrap
├── shell              # Tokenizer + pipeline executor (|, &&, ||, >, >>)
├── vfs                # In-memory virtual filesystem
├── world              # Missions, players, NPCs, combat, economy, campaign
├── scripts            # Sandboxed scripting engine
├── ui                 # Terminal UI components (banners, themes, progress meters)
├── protocol           # Shared types (Mode, MissionStatus, MailMessage, etc.)
└── admin              # Admin tooling

ssh-hunt/tests/        # Integration test suite (93 regression tests)
```

The game server is a custom `russh`-based SSH daemon. All gameplay commands execute against:
- An in-memory **virtual filesystem** (VFS) — not the host filesystem
- A **simulated world state** — missions, NPCs, economy, duels
- A **sandboxed script engine** — player scripts cannot access host resources
- **PostgreSQL** — persistent player data, mission progress, leaderboard

## Security Guarantees

SSH-Hunt is designed for hostile internet exposure.

Hard defensive guarantees:
- No `std::process::Command` or `tokio::process::Command` in game server paths
- No host filesystem access outside mounted runtime data
- No host service control or breakout command path
- Breakout/probing attempts trigger immediate **permanent zero + disconnect**
- `#![forbid(unsafe_code)]` on all gameplay crates

## Deployment and Ops

```bash
make up                     # start all services
make down                   # stop all services
make ps                     # container status
make logs                   # tail logs
make restart                # restart services
make doctor                 # health check
make firewall-open-24444    # open SSH port in firewalld
make db-migrate             # run database migrations
make db-seed                # seed initial data
make test                   # run full test suite
make backup                 # backup player data
```

Cloudflare Tunnel target: `ssh://localhost:24444`
If inside Docker network: `ssh://ssh-hunt:22222`

## CI/CD and Security Pipeline

All CI runs on **self-hosted runners** managed by [haskell-ci-orchestrator](https://github.com/jalsarraf0/haskell-ci-orchestrator). The unified pipeline (`.github/workflows/ci.yml`) includes:

| Job | What it does | Trigger |
|-----|-------------|---------|
| **Lint** | `cargo fmt --check` + `cargo clippy -D warnings` | Every push/PR |
| **Test** | `cargo test --workspace` (158+ tests) | Every push/PR |
| **Security** | gitleaks, cargo audit, cargo deny, trivy, CodeQL, osv-scanner | Every push/PR |
| **SBOM** | Syft SPDX + CycloneDX generation | main branch |
| **Docker** | Build + push game server image | main + tags |
| **Release** | SHA256 checksums, build provenance attestation, GitHub Release | Tags only |

Pipeline features:
- Concurrency groups cancel in-progress runs on same ref
- Cargo registry/git/target caching for fast rebuilds
- Self-hosted runners with host networking for service container access
- CPU budget auto-divided across 5 runner containers (1 persistent + 4 ephemeral)
- Weekly scheduled run (Monday 04:00 UTC) for dependency freshness

Runner setup:
```bash
cp .env.runner.example .env.runner
make runner-up              # start self-hosted runners
make runner-logs            # tail runner output
```

## Configuration and Secrets

Environment template: `.env.example`

| Variable | Default | Description |
|----------|---------|-------------|
| `SSH_HUNT_PORT` | `24444` | Host-published SSH port |
| `SSH_HUNT_LISTEN` | `0.0.0.0:22222` | Container listen address |
| `DATABASE_URL` | `postgres://ssh_hunt:...` | PostgreSQL connection |
| `HIDDEN_OPS_PATH` | `/data/secrets/hidden_ops.yaml` | Secret missions config |
| `SSH_HOST_KEY_PATH` | `/data/secrets/ssh_host_ed25519` | Persistent host key |

Runtime secrets (not committed): `admin.yaml`, `hidden_ops.yaml`, `ssh_host_ed25519`

## Troubleshooting

**Service not reachable:** `make ps` → `ss -ltnp | rg 24444` → `make doctor` → `make firewall-open-24444`

**Connection refused:** Containers stopped, missing `.env`, or NAT/port-forward disabled.

**NetCity locked:** Complete `keys-vault` + one starter mission, then reconnect with registered SSH key.

**Host key warning:** Expected only on first deploy. Key persists at `/data/secrets/ssh_host_ed25519`.

**Cross-terminal compatibility:** Server normalizes CRLF, handles CR/LF/CRLF Enter variants, ignores escape sequences, auto-falls back to ASCII frames on narrow terminals.

## Contributing

- Read `docs/GAMEPLAY.md`, `docs/SECURITY.md`, `docs/DEPLOYMENT.md`, and `docs/SELF_HOSTED_RUNNER.md`
- Follow `CODE_OF_CONDUCT.md`
- Run `make test` before opening a PR
- CI gate: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

## License

Dual-licensed under:
- MIT (`LICENSE-MIT`)
- Apache-2.0 (`LICENSE-APACHE`)

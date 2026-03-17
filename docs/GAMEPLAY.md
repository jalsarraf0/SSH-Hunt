# Gameplay Guide

## Modes

- `SOLO TRAINING SIM` (green): onboarding, guided missions, safe progression.
- `MULTIPLAYER NETCITY MMO` (purple neon): shared world, chat, auction, events.
- `REDLINE` (red): 5-minute stress missions with capped bonus rewards.

Mode transitions display text + color shift banners.

## Core Commands

- Navigation/files: `ls`, `cd`, `pwd`, `cat`, `less`, `head`, `tail`, `touch`, `mkdir`, `rm`, `cp`, `mv`
- Text: `echo`, `printf`, `grep`, `find`, `sort`, `uniq`, `wc`, `cut`, `tr`, bounded `sed`, bounded `awk`
- Simulated system: `ps`, `top`, `uname`, `whoami`, `id`, `df`, `free`, `ip`, `ss`
- Game: `help`, `guide`, `briefing`, `tutorial`, `missions`, `accept`, `submit`, `inventory`, `shop`, `auction`, `chat`, `mail`, `party`, `mode`, `gate`, `keyvault`, `settings`, `status`, `events`, `leaderboard`, `scripts`

## Tutorial and Missions

### Tutorial

Run:

```text
guide
guide shell
briefing
tutorial start
```

It teaches prompt structure, core file navigation, pipes, redirection, and mission loop.
The sim also seeds in-world help files you can read with normal shell commands:

- `/missions/rookie-ops.txt`
- `/missions/story-so-far.txt`
- `/data/lore/ghost-rail-dossier.txt`

### Mission 0: KEYS VAULT

Required for MMO unlock.

- Generate client key:
  - `ssh-keygen -t ed25519 -a 64 -f ~/.ssh/ssh-hunt_ed25519`
- Keep private key safe, never share.
- Register public key with:
  - `keyvault register`

### Starter Path Missions

Complete one of:

- `pipes-101`: `grep | wc` pipeline fundamentals
- `log-hunt`: extract mission token from logs
- `redirect-lab`: `>` and `>>` file manipulation
- `dedupe-city`: `sort | uniq` analysis
- `finder`: safe `find` + argument chaining

For mission-specific story + hints:

- `briefing <mission-code>`

Unlock condition for NetCity:

- KEYS VAULT complete
- plus one starter mission complete

## Economy and Auction

Currency: `Neon Chips`.

- `shop list`, `shop buy <item>`
- `auction list`, `auction sell`, `auction bid`, `auction buyout`
- listing fees, taxes, floors, and rate limits prevent abuse.

## Scripts and Script Market

- `scripts run <name>` executes Rhai sandbox scripts.
- `scripts market` shows curated scripts for non-coders.
- Cooldowns and diminishing returns prevent progression bypass.

## Player Status and Events

- `status` shows wallet, reputation, streak, mode/tier, achievements, and NetCity gate state.
- `gate` shows exact NetCity unlock blockers and checklist.
- `events` shows active/upcoming world events with countdown timing.
- `leaderboard [N]` shows top non-zeroed players by reputation and wallet.

## Achievements

Style bonuses reward clever but safe command composition.

- `Pipe Dream`
- `Gremlin Grep`
- `Redirection Wizard`

## Social

- Chat channels: `chat global|sector|party <msg>`
- Parties: `party invite|join|leave`
- Mail: `mail inbox|send`

## PvP Combat

- `pvp roster`
- `pvp challenge <username>`
- `pvp attack`
- `pvp defend`
- `pvp script <name>`

Difficulty tiers:

- `Noob`
- `Gud`
- `Hardcore`

Set tier with: `tier noob|gud|hardcore`.
Hardcore accounts are zeroed after 3 deaths.

## REDLINE Accessibility

Flash is ON by default in REDLINE.
Disable with:

- `settings flash off`
- `mode redline --no-flash`

## Breakout Defense Policy

Any command attempting host breakout/probing (for example host shell invocation, privilege escalation, docker socket probing, external runtime/process escape patterns) triggers immediate permanent zeroing and forced disconnect.

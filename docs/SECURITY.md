# Security Model

## Threat Model

SSH-Hunt is intentionally exposed to hostile public traffic. Primary concerns:

- auth abuse / flooding,
- chat abuse and griefing,
- economy exploitation,
- script sandbox escapes,
- container breakout attempts.

## Core Guarantees

- Simulated shell only, no host shell execution.
- No real network probing/scanning code paths.
- No host filesystem access for gameplay logic.
- Data access limited to Postgres and `/data` runtime config/log scope.

## Container Hardening

`ssh-hunt` service is configured with:

- `read_only: true`
- `tmpfs` for `/tmp` and `/run`
- `cap_drop: [ALL]`
- `no-new-privileges:true`
- non-root runtime user
- memory/CPU/PID limits

Postgres is only on an internal Docker network and has no host-published port.

## Rate Limiting and Abuse Controls

- per-IP and per-session command rate limit,
- chat throttle and moderation tools,
- auction listing throttles + floor/fee controls,
- host-breakout intrusion guard with permanent zero + disconnect on detection.

## Script Sandbox

Rhai scripts are restricted by policy:

- max source size,
- max operations,
- max runtime,
- max output size,
- allow-listed game APIs only,
- no filesystem/network APIs.

## Admin Controls

Admin identity is injected privately at runtime via `/data/secrets/admin.yaml`.
No privileged identities are hardcoded in repository source.

Optional Telegram relay for hidden-endgame contact is configured privately via `/data/secrets/hidden_ops.yaml`.
Relay payloads use generated in-game aliases and avoid player PII fields.

## CI Security Evidence

Pipelines run:

- dependency audits (`cargo audit`, `cargo deny`),
- secret scanning (`gitleaks`),
- vuln scans (`trivy`, `osv-scanner`),
- CodeQL static analysis for Rust security paths,
- forbidden-API guard (fails on `std::process::Command` usage),
- SBOM + signed image attestations,
- weekly deep sweep and automated dependency update PRs.

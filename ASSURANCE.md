# Software Assurance

This document describes the CI/CD gates, security scanning, supply chain protections,
and quality controls that guard the SSH-Hunt codebase. Every pull request and push to
`main` must pass these gates before code is merged or deployed.

---

## Table of Contents

- [CI/CD Pipeline Overview](#cicd-pipeline-overview)
- [Code Quality Gates](#code-quality-gates)
- [Security Scanning](#security-scanning)
- [Supply Chain Security](#supply-chain-security)
- [Intrusion Guard Enforcement](#intrusion-guard-enforcement)
- [Concurrency and Runner Controls](#concurrency-and-runner-controls)
- [Running Checks Locally](#running-checks-locally)
- [How This Protects Against Regressions](#how-this-protects-against-regressions)

---

## CI/CD Pipeline Overview

SSH-Hunt uses six GitHub Actions workflows that collectively enforce code quality,
security posture, and supply chain integrity:

| Workflow | File | Trigger | Purpose |
|---|---|---|---|
| **CI** | `ci.yml` | push, PR | Build, lint, test, coverage |
| **Security** | `security.yml` | push, PR | Vulnerability scanning, secret detection, intrusion guards |
| **CodeQL** | `codeql.yml` | push, PR, weekly schedule | Semantic code analysis for security and quality bugs |
| **Deep Security Sweep** | `deep-security-sweep.yml` | weekly schedule, manual | Full-depth vulnerability scan + automated dependency update PR |
| **Docker Build and Publish** | `docker.yml` | push to main, tags | Build, sign, publish container image with SBOM and provenance |
| **SBOM (Source Level)** | `sbom.yml` | push, PR | Generate and validate CycloneDX SBOMs for all workspace crates |

All workflows use `vars.SSH_HUNT_RUNNER_LABELS` to target self-hosted runners when
configured, falling back to `ubuntu-latest` for forks and clones.

---

## Code Quality Gates

### Formatting

`cargo fmt --all -- --check` enforces consistent Rust formatting across all workspace
crates. PRs with formatting violations are rejected.

### Linting

`cargo clippy --workspace --all-targets --all-features -- -D warnings` treats every
Clippy warning as a hard error. This catches common bugs, performance issues, and
idiomatic violations before review.

### Build Verification

`cargo check --workspace --all-features --locked` verifies that the workspace compiles
with the locked dependency set. The `--locked` flag ensures CI uses the exact
`Cargo.lock` committed to the repository.

### Testing

- **Unit and integration tests**: `cargo test --workspace --all-features --locked` runs
  the full test suite with a live PostgreSQL 16 service container.
- **Regression suite**: `cargo test -p integration_tests --test regression --release --locked`
  runs the regression suite under release profile to catch optimization-sensitive bugs.
- **Database migrations**: `cargo run -p admin -- migrate` and `sqlx migrate info` verify
  that all SQL migrations apply cleanly.
- **Database seeding**: `cargo run -p admin -- seed` validates that seed data loads
  without errors.

### Coverage

`cargo-tarpaulin` with the LLVM engine generates Cobertura XML coverage reports for the
full workspace. Reports are uploaded as build artifacts for review.

### Dependency Review

On pull requests, `actions/dependency-review-action` flags newly introduced dependencies
with known vulnerabilities before merge.

---

## Security Scanning

### cargo-audit

Checks `Cargo.lock` against the RustSec Advisory Database for known vulnerabilities in
dependencies. Runs in both the Security workflow (every push/PR) and the SBOM workflow.

### cargo-deny

Enforces policy rules defined in `ssh-hunt/deny.toml`:

- **Bans**: blocks specific crates or versions known to be problematic.
- **Licenses**: rejects dependencies with incompatible licenses.
- **Sources**: restricts allowed crate registries.

### Gitleaks

Scans the full Git history for accidentally committed secrets (API keys, tokens,
passwords, private keys). Results are uploaded as SARIF to GitHub Security tab.

### Trivy

Aquasecurity Trivy performs filesystem-level vulnerability scanning across the entire
repository, covering Rust dependencies, Dockerfiles, and configuration files. Results
are uploaded as SARIF.

### OSV-Scanner

Google's OSV-Scanner performs recursive vulnerability scanning against the Open Source
Vulnerabilities database, configured via `ssh-hunt/osv-scanner.toml`.

### CodeQL

GitHub CodeQL runs semantic analysis on the Rust codebase using `security-extended` and
`security-and-quality` query suites. Scheduled weekly (Tuesday 03:00 UTC) in addition to
running on every push and PR. Results appear in the GitHub Security tab.

### Deep Security Sweep

A weekly scheduled workflow that runs the full scanner stack (cargo-audit, cargo-deny,
Trivy, OSV-Scanner) at greater depth, including medium-severity findings. It also
automatically creates a dependency update PR with `cargo update -w` output.

---

## Supply Chain Security

### Container Image Signing (cosign)

Every container image pushed to GHCR is signed using Sigstore cosign in keyless mode
(OIDC-backed identity). This provides cryptographic proof that images were built by
this repository's CI pipeline and have not been tampered with.

Verification:

```bash
cosign verify \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp 'github.com/jalsarraf0/SSH-Hunt' \
  ghcr.io/jalsarraf0/ssh-hunt/ssh-hunt:main
```

### SBOM Generation

Two complementary SBOM formats are produced:

- **SPDX** (via `anchore/sbom-action`): generated from the built container image in the
  Docker workflow. Uploaded as a build artifact.
- **CycloneDX** (via `cargo-cyclonedx`): generated from Cargo workspace source in the
  SBOM workflow. One SBOM per workspace crate, validated to ensure all key crates
  (`ssh_hunt_server`, `world`, `shell`, `vfs`) are covered. Retained for 90 days.

### Build Provenance Attestation

`actions/attest-build-provenance` generates SLSA-compatible provenance attestations for
every container image build. Attestations are pushed to the GHCR registry alongside the
image, providing an auditable record of what was built, when, and by which workflow run.

### Locked Dependencies

All CI build and test steps use `--locked` to ensure the exact dependency versions from
`Cargo.lock` are used. Any dependency drift between local development and CI is caught
immediately.

---

## Intrusion Guard Enforcement

SSH-Hunt is designed for hostile internet exposure. The Security workflow enforces
critical runtime safety invariants on every push and PR:

### Forbidden API Check

`rg "std::process::Command|tokio::process::Command" ssh-hunt/crates` scans all crate
source for process-spawning APIs. If any match is found, the build fails. This prevents
accidental introduction of host command execution paths that could allow player input to
reach the host OS.

### Intrusion Guard Verification

The workflow verifies that three critical defense functions remain present in the server
binary source:

- `fn escape_attempt_reason` -- classifies breakout attempts
- `ban_forever(` -- permanently bans offending accounts
- `INTRUSION DETECTED` -- the log/alert sentinel string

If any of these are removed or renamed, CI fails. This guards against accidental
removal of the intrusion detection and response system.

### Self-Hosted Runner Directive

`./scripts/verify-self-hosted-runner-directive.sh` runs on every push to verify that
the runner configuration matches the repository's security policy.

---

## Concurrency and Runner Controls

### Concurrency Groups

Each workflow uses concurrency groups scoped to `${{ github.ref }}` with
`cancel-in-progress: true`. This ensures that:

- Redundant runs on the same branch are cancelled automatically.
- CI resources are not wasted on superseded commits.
- Results always reflect the latest pushed state.

### Least-Privilege Permissions

Every workflow declares explicit `permissions` blocks scoped to the minimum required:

- CI: `contents: read`
- Security: `contents: read`, `security-events: write`
- CodeQL: `contents: read`, `actions: read`, `security-events: write`
- Docker: `contents: read`, `packages: write`, `id-token: write`, `attestations: write`

### Runner Selection

All jobs use `vars.SSH_HUNT_RUNNER_LABELS` for runner targeting. When set to
`["self-hosted","linux","x64","ssh-hunt"]`, jobs run on dedicated infrastructure with
CPU budgets managed by `scripts/refresh-runner-cpu-budget.sh`. When unset, jobs fall
back to `ubuntu-latest` for portability.

---

## Running Checks Locally

Contributors can run the same checks that CI enforces before pushing:

```bash
cd ssh-hunt

# Formatting
cargo fmt --all -- --check

# Linting
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Build check
cargo check --workspace --all-features --locked

# Full test suite (requires PostgreSQL at DATABASE_URL)
cargo test --workspace --all-features --locked

# Regression suite (release profile)
cargo test -p integration_tests --test regression --release --locked

# Dependency audit
cargo audit --ignore RUSTSEC-2023-0071

# License and policy check
cargo deny check --config deny.toml --hide-inclusion-graph bans licenses sources

# Coverage
cargo tarpaulin --engine llvm --workspace --all-features --timeout 120

# Secret scan (requires gitleaks)
gitleaks detect --source . --redact --verbose

# SBOM generation (requires cargo-cyclonedx)
cargo cyclonedx --format json --spec-version 1.4
```

For a quick Docker-based full stack check:

```bash
docker compose up --build -d
make test
make doctor
```

---

## How This Protects Against Regressions

| Risk | Mitigation |
|---|---|
| Formatting drift | `cargo fmt --check` on every PR |
| Lint regressions | `clippy -D warnings` on every PR |
| Test failures | Full workspace + regression suite on every PR |
| Known CVEs in dependencies | cargo-audit, Trivy, OSV-Scanner on every PR |
| License violations | cargo-deny policy enforcement on every PR |
| Leaked secrets | Gitleaks full-history scan on every PR |
| Semantic security bugs | CodeQL with extended queries on every PR + weekly |
| Stale dependencies | Weekly deep sweep + automated update PR |
| Host command injection | Forbidden API grep on every PR |
| Intrusion guard removal | Sentinel function verification on every PR |
| Tampered container images | Cosign keyless signing + provenance attestation |
| Unknown dependency contents | Dual-format SBOM (SPDX + CycloneDX) per release |
| Dependency version drift | `--locked` flag on all cargo commands in CI |
| Wasted CI resources | Concurrency groups with cancel-in-progress |
| Over-privileged workflows | Explicit least-privilege permission blocks |

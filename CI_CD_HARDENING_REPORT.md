# CI/CD Hardening Report -- SSH-Hunt

**Repository:** `jalsarraf0/SSH-Hunt`
**Date:** 2026-03-14
**Branch:** `main`

---

## Pre-Existing CI/CD Infrastructure

SSH-Hunt had a mature, comprehensive CI/CD pipeline already in place with six workflows:

| Workflow | Status | Notes |
|---|---|---|
| CI (`ci.yml`) | Fully operational | fmt, clippy, build, migrate, test, regression, tarpaulin coverage |
| Security (`security.yml`) | Fully operational | cargo-audit, cargo-deny, gitleaks, Trivy, OSV-Scanner, intrusion guards |
| CodeQL (`codeql.yml`) | Fully operational | Rust semantic analysis, weekly schedule + PR triggers |
| Deep Security Sweep (`deep-security-sweep.yml`) | Fully operational | Weekly full-depth scan + automated dependency update PR |
| Docker Build and Publish (`docker.yml`) | Fully operational | GHCR push, cosign signing, SPDX SBOM, build provenance attestation |
| SBOM (`sbom.yml`) | Fully operational | CycloneDX source-level SBOMs, per-crate validation |

### Security Scanning Tools

- cargo-audit (RustSec Advisory DB)
- cargo-deny (license, ban, and source policy)
- Gitleaks (secret detection with SARIF upload)
- Trivy (filesystem vulnerability scan with SARIF upload)
- OSV-Scanner (Google OSV database, recursive scan)
- CodeQL (semantic analysis, security-extended + security-and-quality)

### Supply Chain Security

- Cosign keyless image signing (Sigstore OIDC)
- SPDX SBOM from container image (anchore/sbom-action)
- CycloneDX SBOMs from Cargo source (cargo-cyclonedx)
- Build provenance attestation (actions/attest-build-provenance, SLSA-compatible)
- Locked dependencies (`--locked` on all cargo commands)

### Quality Controls

- PostgreSQL 16 service container for integration tests
- Regression suite under release profile
- Tarpaulin coverage with artifact upload
- Dependency review on PRs
- Forbidden API detection (std::process::Command, tokio::process::Command)
- Intrusion guard sentinel verification
- Self-hosted runner directive enforcement
- Concurrency groups with cancel-in-progress
- Least-privilege permissions on all workflows

---

## What Was Added

| Item | Type | Description |
|---|---|---|
| `ASSURANCE.md` | New file | Comprehensive software assurance document covering all CI/CD gates, security scanning, supply chain protections, intrusion guards, and local check instructions |
| `CI_CD_HARDENING_REPORT.md` | New file | This report |

### Deferred: SBOM Badge in README.md

README.md has 5 badges (CI, Security, CodeQL, Deep Sweep, Docker) but is missing a
badge for the SBOM workflow. The following line should be added after the Docker badge
when the current README working changes are committed:

```markdown
[![SBOM](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/sbom.yml/badge.svg)](https://github.com/jalsarraf0/SSH-Hunt/actions/workflows/sbom.yml)
```

This was deferred because README.md has unrelated development changes in progress.

---

## What Was NOT Changed

- No workflow files were modified
- No source code was modified
- No configuration files were modified
- 8 dirty working tree files (development WIP) were not touched

---

## Verdict

**SSH-Hunt is the gold standard for CI/CD hardening in this fleet.** The pipeline covers
all critical dimensions: code quality, security scanning, supply chain integrity,
runtime safety enforcement, and operational controls. No workflow modifications were
needed. The only gap was documentation -- the new `ASSURANCE.md` makes the security
posture explicit and auditable, and the SBOM badge completes README badge coverage for
all six workflows.

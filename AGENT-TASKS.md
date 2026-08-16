# Spindle — Agent Get-Well Tasks

> **Based on:** [AUDIT-REPORT.md](https://github.com/o3willard-AI/Spindle/blob/main/AUDIT-REPORT.md) by Release Engineer (GLM 5.2)
> **Date:** 2026-08-11
> **Goal:** Address all P0–P2 findings. Target maturity: 4.0/5 (from current 1.4/5).

---

## Before Agents Start (Stephen)

| # | Action | Effort |
|---|--------|--------|
| S-1 | Rotate GitHub PAT, replace git remote URL with clean HTTPS or SSH | 30min |
| S-2 | Rotate `CHANGE_ME` on live DB (.101), store new password in KeePass | 15min |
| S-3 | After rotation, provide agents with new DB URL and PAT | — |

---

## Phase 1: Stabilize (This Week) — All agents in parallel

### Wave 1A — Release Engineer: Dependencies + Security

| # | Task | Priority | Effort | Exit Criterion |
|---|------|----------|--------|----------------|
| S-1 | Upgrade `quick-xml` to ≥0.41.0 (fixes 2 HIGH CVEs in SAML/Dex path) | **P0** | 1h | `cargo audit` shows 0 quick-xml advisories |
| S-2 | Upgrade `sqlx` to ≥0.8.1 (fixes RUSTSEC-2024-0363) | **P0** | 2h | `cargo audit` shows 0 sqlx advisories |
| S-3 | Upgrade `object_store` to ≥0.10.2, `rustls-webpki` chain | P1 | 1h | `cargo audit` shows 0 advisories |
| S-4 | Replace 3 unmaintained deps (paste, proc-macro-error2, rustls-pemfile) | P1 | 2h | `cargo audit` shows 0 unmaintained |
| S-5 | Align dependency versions: ed25519-dalek→3.0, parquet/arrow→54, base64→0.22 | P2 | 2h | `cargo tree -d` shows no duplicate major versions |
| S-6 | Add `cargo-deny.toml` with license/supply-chain/ban rules | P1 | 1h | `cargo deny check` passes |
| S-7 | Remove hardcoded credentials from source files (5 locations) — requires Stephen's new DB password | **P0** | 2h | `grep -rn 'CHANGE_ME' --include='*.rs' .` returns 0 |

### Wave 1B — Core Developer: CI/CD + Infrastructure

| # | Task | Priority | Effort | Exit Criterion |
|---|------|----------|--------|----------------|
| M-1 | Create `.github/workflows/ci.yml`: fmt, clippy, test, audit, deny | **P0** | 3h | CI runs on every PR; all gates must pass |
| M-2 | Add `gitleaks detect` to CI as required check | **P0** | 1h | Secret scan runs on every PR |
| M-3 | Add `cargo llvm-cov` with diff-coverage gate at 80% to CI | P1 | 1h | Coverage report in CI artifacts |
| M-4 | Add branch protection rules on `main`: require PR review, CI pass, signed commits | P1 | 30min | Branch protection rules enforced |
| M-5 | Add `dependabot.yml` for cargo + docker + github-actions | P2 | 30min | Dependabot PRs arriving weekly |
| M-6 | Fix clippy hard error: `approx_constant` in `spindle-api/src/filter.rs:445` | P2 | 15min | `cargo clippy` exits 0 |
| M-7 | SHA-pin all GitHub Actions (not tags) | P2 | 30min | `actionlint` passes |

### Wave 1C — Deployment Engineer: Code Cleanup + Documentation

| # | Task | Priority | Effort | Exit Criterion |
|---|------|----------|--------|----------------|
| K-1 | Delete `spindle-server/src/auth.rs` (1,828 LOC dead code) — remove from module tree | P2 | 1h | `auth.rs` deleted; workspace compiles |
| K-2 | Remove 3 stub crates from workspace: `spindle-corpus-capture`, `spindle-tokens`, `spindle-ingest` | P2 | 30min | 3 directories deleted; workspace has 23 crates |
| K-3 | Remove committed binary artifacts: tarball, .pyc, evidence output — update `.gitignore` | P2 | 30min | `git ls-files | grep -E 'tar.gz|pyc|evidence-collector/output'` returns 0 |
| K-4 | Create `AGENTS.md` at repo root: project overview, crate map, build/test commands, security rules, migration conventions | P1 | 2h | `AGENTS.md` ≥100 lines, covers 7 conventions |
| K-5 | Create root `README.md`: what Spindle is, architecture, quick start, links to docs | P2 | 1h | `README.md` exists |
| K-6 | Fix `AlwaysUpChecker` → real health checks: DB `SELECT 1`, storage write/read/delete test, Dex `/healthz` | **P0** | 2h | Stop DB → `/health` returns 503 |
| K-7 | Replace `InMemory*Store` production fallback with hard fail: server exits 1 if DB unreachable | **P0** | 1h | `SPINDLE_PRODUCTION=1` + no DB → exit 1 |
| K-8 | Add `spindle-server --version` flag (commit SHA + build date) | P2 | 30min | `--version` prints SHA + date |

### Wave 1D — Release Engineer (after deps): Critical Security Fixes

| # | Task | Priority | Effort | Exit Criterion |
|---|------|----------|--------|----------------|
| S-8 | Fix P0-3: replace `check_role_authorization()` (header-based) with JWT-derived role middleware | **P0** | 1d | `X-User-Role: admin` header no longer grants admin; role from JWT claims |
| S-9 | Fix P1-3: bind scope filter params — refactor `build_scope_filter()` to use `QueryBuilder` with `.bind()` | P1 | 1d | `grep -rn '_params' spindle-store/src/` returns 0 |
| S-10 | Add rate limiting on `/v1/auth/*` endpoints (governor: 5/min login, 3/min register) | P1 | 2h | 429 returned after threshold |
| S-11 | Change `SessionConfig::default()` to require `SPINDLE_JWT_SECRET` env var (panic if missing in prod) | **P0** | 1h | No hardcoded `super-secret-key-change-in-production` in source |

---

## Phase 2: Harden (Next Week) — After Phase 1 complete

### Core Developer: CI Hardening

| # | Task | Priority | Effort | Dependencies |
|---|------|----------|--------|-------------|
| M-8 | Fix all 136 clippy warnings across workspace | P2 | 3h | None |
| M-9 | Set `#![deny(clippy::all)]` in workspace lib | P2 | 30min | M-8 complete |
| M-10 | Add TLS support: `axum-server` + `rustls`, config options, required in production mode | **P0** | 1d | None |

### Release Engineer: Observability + Testing

| # | Task | Priority | Effort | Dependencies |
|---|------|----------|--------|-------------|
| S-12 | Wire `MetricsRegistry` counters to actual request handlers | P1 | 2h | None |
| S-13 | Add `tower-http::trace::TraceLayer` for HTTP request tracing | P2 | 1h | None |
| S-14 | Define SLOs in `docs/slo.md`: ingest p99<500ms, query p99<200ms, 99.9% uptime, >99% success | P2 | 30min | None |
| S-15 | Write 4 characterization tests for documented broken behaviors (.json.gz not compressed, metrics blind, DB→in-memory fallback, AlwaysUpChecker) | P1 | 2h | None |
| S-16 | Add 16+ worker integration tests (dequeue, parse, filter, store, DLQ, retry, compliance) | P1 | 3h | None |
| S-17 | Add 18+ store integration tests (all CRUD, scope filtering, error paths) | P1 | 2h | None |

### Deployment Engineer: Documentation + Operations

| # | Task | Priority | Effort | Dependencies |
|---|------|----------|--------|-------------|
| K-9 | Fix `.json.gz` archive naming: either compress with `flate2` or rename to `.json`. ADR-003. | P1 | 1h | None |
| K-10 | Document rollback procedure: `docs/operator/rollback.md` | P2 | 1h | None |
| K-11 | Add `down.sql` to all 28 migrations (or document backup-restore rollback for destructive ones) | P2 | 3h | None |
| K-12 | Create ADR-001 (security baseline), ADR-002 (dead code removal), ADR-003 (archive compression decision) | P2 | 2h | None |
| K-13 | Pin Docker images: no `:latest` tags in `docker-compose.yml` or `Dockerfile` | P2 | 30min | None |
| K-14 | Add `make sbom` target (cargo-cyclonedx) | P2 | 30min | None |

---

## Phase 3: Modernize (Weeks 3+) — After Phases 1+2

### All Agents (coordinated, sequential on spindle-server)

| # | Task | Priority | Effort | Assignee |
|---|------|----------|--------|----------|
| C-1 | Deduplicate `NodeStore` trait: merge store trait with server trait (no name collision) | P2 | 2d | Release Engineer |
| C-2 | Systematic `unwrap()` → `?` conversion: target ≤100 production unwraps (89% reduction from 1130) | P2 | 1w | Release Engineer |
| C-3 | Replace 6 `panic!()` calls with `Result` returns | P2 | 1d | Release Engineer |
| C-4 | Add `// SAFETY:` comments to 6 `unsafe impl Send/Sync` | P2 | 1d | Release Engineer |
| C-5 | Re-base migrations: capture live schema → clean sequential set with `up.sql`/`down.sql` | P2 | 3d | Release Engineer |
| C-6 | Fix airgap config: remove SQLite reference, use PostgreSQL | P2 | 1h | Deployment Engineer |
| C-7 | Add `.env.example` with all required env vars documented | P2 | 1h | Deployment Engineer |

---

## Dependency Graph

```
S-1 (Stephen: rotate PAT)
S-2 (Stephen: rotate DB password)
  │
  ├─► S-7 (Release Engineer: remove hardcoded creds from source)
  │
  ├─► K-1 (Deployment Engineer: delete auth.rs)
  ├─► K-2 (Deployment Engineer: remove stub crates)
  ├─► K-3 (Deployment Engineer: remove binary artifacts)
  ├─► K-4 (Deployment Engineer: AGENTS.md)
  ├─► K-5 (Deployment Engineer: README.md)
  │
  ├─► S-1..S-6 (Release Engineer: dependency upgrades)
  │     └─► S-8 (role escalation fix)
  │     └─► S-9 (scope filter fix)
  │     └─► S-10 (auth rate limiting)
  │     └─► S-11 (JWT secret env)
  │
  └─► M-1..M-7 (Core Developer: CI/CD + clippy)
        └─► M-8 (fix 136 clippy warnings)
        └─► M-10 (TLS)
```

## Quick Reference: Per-Agent Summary

### Release Engineer (backend)
- **P0:** quick-xml upgrade, sqlx upgrade, role escalation fix, JWT secret, remove creds
- **P1:** object_store/rustls upgrade, unmaintained deps, version alignment, scope filter, rate limiting, metrics wiring, characterization tests, worker tests, store tests
- **P2:** cargo-deny, unwrap reduction, panic removal, unsafe docs, migration rebase, dedup

### Core Developer (infra)
- **P0:** CI pipeline, gitleaks in CI
- **P1:** coverage gate, branch protection, TLS
- **P2:** clippy fix (136 warnings), dependabot, SHA-pinned actions, SLOs

### Deployment Engineer (ops/docs)
- **P0:** real health checks, no in-memory fallback, archive naming fix
- **P1:** AGENTS.md, dead code removal, stub crate deletion, binary artifact cleanup
- **P2:** README, ADRs (3), rollback doc, migration down.sql, Docker pinning, SBOM, airgap fix

### Stephen (project lead)
- **P0:** rotate GitHub PAT, rotate DB password
- **P1:** `.env.example`, commit signing setup

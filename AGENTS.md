# AGENTS.md — Spindle Engineering Conventions

> This file is the authoritative guide for any AI agent, developer, or contributor working on the Spindle codebase. All agents are expected to read this before making changes.

## 1. Project Overview

**Spindle** is a fleet infrastructure observability platform. It ingests Cinc Client data-collector events and Cinc Auditor compliance reports, stores them in PostgreSQL, archives raw payloads to local filesystem or S3/MinIO, and exposes a unified REST API for querying node inventory, run history, compliance reports, cookbooks, waivers, and resource-event aggregates.

### Core Architecture

```
CINC Clients
    │  data-collector events / auditor reports
    ▼
spindle-server (:3000)          ← axum HTTP server
    │
    ├─── ingest ────┐  bearer token auth (SPINDLE_INGEST_TOKEN)
    │               ├── raw archive → spindle-rawarchive (local FS / S3)
    │               ├── idempotency → Postgres (or in-memory for dev)
    │               └── job queue   → Postgres ingest_queue table
    │
    ├─── query API ──┐  bearer token auth (SPINDLE_INGEST_TOKEN)
    │               ├── nodes        → spindle-store (SqlxNodeStore)
    │               ├── runs         → spindle-store (SqlxRunStore)
    │               ├── compliance   → spindle-store (SqlxComplianceStore)
    │               ├── cookbooks    → in-memory store
    │               ├── waivers      → in-memory store + audit log
    │               ├── resource-events → rollup store (in-memory)
    │               └── health       → real DB/storage/Dex probes
    │
    ├─── auth (JIT) ── OIDC login via Dex → provisions users into PostgreSQL
    └─── metrics / /health/metrics → Prometheus-format

spindle-worker       ← async pipeline: parse → normalize → filter → SQL insert
spindle-cli          ← human/operator interface (table + JSON modes)
```

### Crate Map (23 crates)

| Crate | Purpose |
|---|---|
| `spindle-server` | Axum HTTP server: ingest, query API, health, auth |
| `spindle-worker` | Async pipeline worker: process archived payloads |
| `spindle-cli` | Operator CLI: query, inspect, trigger pipeline |
| `spindle-config` | Figment-based layered config (defaults → TOML → env) |
| `spindle-obs` | Observability helpers: tracing, logging tiers |
| `spindle-error` | Shared error types |
| `spindle-rawarchive` | Raw payload archive: local FS + S3 backends |
| `spindle-store` | Typed PostgreSQL stores with scope enforcement |
| `spindle-pipeline` | Shared pipeline types and normalization |
| `spindle-api` | Shared API types: filters, pagination, sort |
| `spindle-identity` | Identity mapping rules (JIT OIDC claim → role/scope) |
| `spindle-authz` | Scope, role, and authorization types |
| `spindle-signing` | PGP/GPG content signing |
| `spindle-compliance` | Compliance report and control evaluation |
| `spindle-archive` | Archive management and retention |
| `spindle-shutdown` | Graceful shutdown coordination |
| `spindle-migrate` | Database migration runner |
| `spindle-dex` | Dex identity provider integration + health checks |
| `spindle-saml` | SAML assertion handling |
| `spindle-bench` | Benchmarking harness |
| `spindle-dashboard` | Web UI dashboard |
| `mcp-server` | MCP (Model Context Protocol) server |
| `spindle-mcp` | Spindle-specific MCP tools |

## 2. Build & Test Commands

### Prerequisites
- Rust stable toolchain (`rustup toolchain install stable`)
- Components: `rustfmt`, `clippy`
- PostgreSQL 15+ (local or Docker via `docker compose up -d`)
- Docker + Docker Compose (for test infrastructure)

### Workspace-wide commands

```bash
# Format all crates
cargo fmt --all

# Lint all crates
cargo clippy --all-targets -- -D warnings

# Build entire workspace
cargo build --release

# Run all tests (unit + integration)
cargo test --workspace

# Run tests for a single crate
cargo test -p spindle-server

# Check for dependency vulnerabilities
cargo audit

# Check for license violations
cargo deny check
```

### Test infrastructure
```bash
# Start PostgreSQL + MinIO + Keycloak
make test-up

# Reset (destroy + rebuild)
make test-reset

# Stop everything
make test-down

# Execute shell in postgres
make test-exec-db
```

### Key environment variables
| Variable | Default | Description |
|---|---|---|
| `SPINDLE_DATABASE_URL` (or `DATABASE_URL`) | `postgres://spindle:CHANGE_ME@localhost:5432/spindle` | PostgreSQL connection |
| `SPINDLE_INGEST_TOKEN` | `spindle-dev-token` | Bearer token for ingest + API |
| `SPINDLE_ARCHIVE_DIR` | `/var/lib/spindle/archive` | Raw archive root directory |
| `SPINDLE_CONFIG` | `~/.config/spindle/config.toml` | Config file path |
| `SPINDLE_PRODUCTION` | (unset) | Set to `1` to enforce DB-required startup |
| `SPINDLE_LOG_LEVEL` | `operational` | `operational` (L1), `diagnostic` (L2), `debug` (L3) |
| `SPINDLE_LOG_TARGET` | `json` | `stdout` for human-readable, `json` for log shipper |
| `RUST_LOG` | (unset) | Per-crate override, e.g. `spindle_server=debug` |

### Development mode vs Production mode
- **Dev mode** (default): `SPINDLE_PRODUCTION` unset. Server starts with in-memory fallback stores if DB is unreachable. Intended for local development and testing.
- **Production mode** (`SPINDLE_PRODUCTION=1`): DB connection is **required**. If the database cannot be reached at startup, the server exits with code 1. No silent in-memory fallback.

## 3. Code Style & Conventions

### Rust edition and formatting
- Edition: 2021 (see `rust-toolchain.toml`)
- All code must be `cargo fmt` compliant
- All code must pass `cargo clippy -- -D warnings`
- Use `tracing` for logging (not `println!` in library code; `println!` is acceptable in `main.rs` for startup messages)

### Module structure
- Each crate has `src/main.rs` or `src/lib.rs` as the entry point
- Public modules are declared with `pub mod` in `lib.rs`
- Integration tests live in `tests/` directory (black-box, using public API)
- Unit tests live in `#[cfg(test)] mod tests` at the bottom of each source file

### Error handling
- Use `thiserror` for all error types
- Define a `Result<T>` alias per crate (e.g., `pub type Result<T> = std::result::Result<T, MyError>`)
- Never `unwrap()` or `panic!()` in non-test library code
- Propagate errors with `?` operator

### Async runtime
- Tokio with `full` features
- All I/O is async: HTTP (`axum`), DB (`sqlx`), storage (`object_store`)
- Use `tokio::time::timeout` for all health checks and external calls (5s default)

### HTTP API conventions
- Routes are assembled in `main.rs` under `run_server()`
- All API endpoints (except `/health`, `/metrics`) require `Bearer <token>` auth via `require_bearer_token` middleware
- Routes use `axum` 0.7 with `:param` syntax (NOT `{param}`) — matchit 0.7.3
- Routes are grouped by domain: `/ingest/`, `/v1/nodes`, `/v1/runs`, `/v1/compliance/*`, `/v1/waivers`, `/v1/cookbooks`, `/v1/resource-events`, `/v1/health`
- OpenAPI spec auto-generated via `utoipa` — update `#[derive(ToSchema)]` on response types
- Swagger UI at `/docs`, spec at `/openapi.json`

### Request tracing
- Every request gets an `X-Request-ID` header (generated if absent)
- Request logging middleware wraps all routes (L1: method/path/status/latency, L2: debug, L3: trace)

## 4. Database Migrations & Schema

### Migration locations
- `migrations/` — numbered migration directories (`001_schema_version`, `002_corpus`, etc.)
- Each migration directory contains `up.sql` and optionally `down.sql`
- Run via `spindle-migrate` crate or `cargo run -p spindle-migrate`

### Key tables
- `nodes` — node inventory (refactored in migration 020)
- `runs` — run history with start/end timestamps
- `resource_events` — per-resource change tracking
- `compliance_profiles` / `compliance_reports` — Cinc Auditor results
- `cookbooks` / `cookbook_versions` — cookbook inventory
- `waivers` / `waiver_audit` — compliance waivers with audit trail
- `raw_archive` — metadata for archived payloads
- `ingest_queue` / `idempotency` — ingest pipeline tracking
- `users` / `user_roles` / `sessions` — JIT-provisioned auth entities
- `tokens` — API tokens

### Migration conventions
- Never reorder or renumber existing migrations
- Each migration must be idempotent and reversible (down.sql)
- Foreign keys use `ON DELETE CASCADE` unless soft-delete is needed
- Indexes added in the same migration that creates the columns they reference

## 5. Health Checks & Readiness

### Health endpoint: `GET /v1/health`
Returns aggregate health of three subsystems in parallel (5s timeout each):

| Subsystem | Checker | Probe |
|---|---|---|
| Database | `DbHealthChecker` | `SELECT 1` via sqlx pool |
| Storage | `StorageHealthChecker` | Write → read → delete round-trip on archive root |
| Dex | `DexHealthChecker` | HTTP GET to `{issuer_url}/.well-known/openid-configuration` |

Response: HTTP 200 when all `Up`, HTTP 503 when any subsystem is `Down` or `Degraded`.
Cached for 5 seconds. Prometheus metrics at `/v1/health/metrics`.

### Readiness vs Liveness
- `/v1/health` serves as both liveness and readiness probe
- In production mode (`SPINDLE_PRODUCTION=1`), the server will not start if the database is unreachable — there is no silent fallback to in-memory stores
- The health endpoint's database checker performs a real `SELECT 1` against the pool, NOT a stub

### Testing health checks
- `AlwaysUpChecker`, `AlwaysDownChecker`, `DegradedChecker`, and `SlowChecker` are test helpers — used only in `#[cfg(test)]`
- Tests in `spindle-server/src/health.rs` cover: all-up (200), DB-down (503), degraded (503), cache validity, timeout handling, and Prometheus metrics format

## 6. Security Rules

### Secrets management
- **Never** commit secrets to source control. Run `grep -rn 'password\|secret\|token' --include='*.rs' .` before committing
- Database credentials come from `SPINDLE_DATABASE_URL` env var or config file
- Ingest API token comes from `SPINDLE_INGEST_TOKEN` env var
- OIDC client secret comes from config or env
- Use a secrets manager for credential storage.

### Authentication
- Ingest endpoints use constant-time bearer token comparison (`subtle::ConstantTimeEq`)
- JIT OIDC login (via Dex): validates state/nonce, exchanges auth code, provisions user into `users`/`user_roles` tables, issues session token
- Local username/password auth available for development (`spindle-server::local_accounts`)

### Authorization
- Scope-based access control via `spindle_authz::Scope`
- Every store method requires `&Scope` — enforced at compile time
- `compliance-auditor` role → node attributes stripped at the store query level
- Admin-only endpoints (dead-letter queue) require admin role

### Input validation
- Payloads validated for size (≤10 MB default) before parsing
- All SQL queries use sqlx with parameter binding — no string interpolation for user input
- Path traversal protection on archive keys (`validate_key()`)

### Dependency security
- Run `cargo audit` before every merge — zero HIGH/CRITICAL advisories allowed
- `cargo deny` with license rules is configured via `cargo-deny.toml`
- SHA-pin all GitHub Actions (no moving tags in CI)

## 7. Development Workflow

### Branch strategy
- Work on feature branches: `git checkout -b feat/mytask`
- Keep branches short-lived (< 3 days)
- If `git push` fails with "divergent branches": `git fetch origin && git rebase origin/main`
- To start clean: `git stash`, `git fetch origin && git reset --hard origin/main`, then re-apply

### Commit conventions
- Use conventional commits: `feat:`, `fix:`, `chore:`, `docs:`, `refactor:`, `test:`, `build:`
- Reference task IDs: `K-1: delete auth.rs`
- Keep commits focused — one logical change per commit

### PR process
1. `cargo fmt --all`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test --workspace`
4. `cargo audit`
5. Ensure CI passes (all gates must be green)
6. Squash+merge only after approval

### Adding a new crate
1. Create `mycrate/Cargo.toml` with `version.workspace = true`, `edition = "2021"`
2. Add to root `Cargo.toml` `[workspace]` members and internal dependency references
3. Add any new workspace-level dependencies to `[workspace.dependencies]`
4. Run `cargo check -p mycrate`

### Running locally (dev)
```bash
# Terminal 1: start test infra
make test-up

# Terminal 2: run server with dev defaults
cargo run -p spindle-server

# Server listens on http://127.0.0.1:3000
# Health: curl http://127.0.0.1:3000/v1/health
# Docs:   http://127.0.0.1:3000/docs
```

### Running in production
```bash
export SPINDLE_PRODUCTION=1
export SPINDLE_DATABASE_URL="postgres://user:pass@db:5432/spindle"
export SPINDLE_INGEST_TOKEN="your-secret-token"
cargo run -p spindle-server -- --config /etc/spindle/config.toml --version
```

### Debug commands
- `--validate-config`: validate configuration and exit (0=valid, 1=invalid)
- `--process-payload <key>`: one-shot pipeline trigger for a specific archived payload
- `--config <path>`: specify alternate config file
- `--version`: print version info (commit SHA + build date)
- `--help`: print usage

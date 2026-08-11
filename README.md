# Spindle

> Fleet infrastructure observability platform — ingest Chef Infra data-collector events and InSpec compliance reports, store in PostgreSQL, and query via a unified REST API.

## What Is Spindle?

Spindle is a fleet observability platform designed to collect, normalize, and serve infrastructure state from Chef Infra Client run-converge payloads and Cinc InSpec compliance reports. It provides a single REST API for querying nodes, runs, compliance status, cookbooks, waivers, and resource-event aggregates across an entire fleet.

### Key capabilities

- **Ingest**: Accepts Chef Infra Client data-collector events and InSpec JSON reports via HTTP POST with bearer-token authentication
- **Archive**: Raw payloads are written to a local filesystem or S3/MinIO-backed archive before parsing (write-before-parse guarantee)
- **Pipeline**: Asynchronous worker processes archived payloads → parses JSON → normalizes to database schema → applies filtering rules
- **Query API**: RESTful endpoints for nodes, runs, compliance reports, cookbooks, waivers, and resource-event aggregates with scope-based RBAC
- **Health**: Real-time health checks for database, storage, and Dex identity provider with Prometheus metrics
- **Auth**: JIT OIDC login via Dex — validates tokens, provisions users, issues session tokens
- **Docs**: Auto-generated OpenAPI/Swagger UI at `/docs`

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        Clients                              │
│  Chef Clients (.211–.213)  →  Data-collector + InSpec       │
└──────────────────────────┬──────────────────────────────────┘
                           │ POST /ingest/events/data-collector
                           │ POST /ingest/events/inspec
                           ▼
┌─────────────────────────────────────────────────────────────┐
│  spindle-server (:3000)  —  Axum HTTP server  M1-M5         │
│                                                             │
│  ┌──────────┐  ┌──────────┐  ┌───────────┐  ┌──────────┐ │
│  │  Ingest  │  │  Query   │  │    Auth   │  │  Health  │ │
│  │ Pipeline │  │    API   │  │   (JIT)   │  │  Checks  │ │
│  │          │  │          │  │           │  │          │ │
│  │ • Bearer │  │ • /v1/  │  │ • OIDC    │  │ • /v1/   │ │
│  │ • Archive│  │   nodes  │  │ • Dex     │  │   health │ │
│  │ • Queue  │  │ • /v1/  │  │ • Session │  │ • 503   │ │
│  │ • Rate   │  │   runs   │  │   tokens  │  │ • Cache │ │
│  │   limit  │  │ • /v1/  │  │           │  │          │ │
│  └──────────┘  │   comp.  │  └───────────┘  └──────────┘ │
│                │ • /v1/   │                              │
│                │   waive. │                              │
│                │ • /v1/  │                              │
│                │   cbk.  │                              │
│                │ • /v1/  │                              │
│                │  res-ev │                              │
│                └──────────┘                              │
└──────────┬───────────────────────────────────────────────┘
           │
           ├── PostgreSQL (sqlx) ←─── spindle-store (typed stores + scope enforcement)
           └── Raw Archive ←─── spindle-rawarchive (local FS / S3)
           │
           ▼
┌─────────────────────────────────────────────────────────────┐
│  spindle-worker  —  Async pipeline processor                │
│  Parses archived payloads → normalizes → inserts into PG    │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  Operators / Programs / Agents                              │
│  CLI (table/JSON)  •  MCP (AI agents)  •  Web UI (spindle- │
│  dashboard)                                              │
└─────────────────────────────────────────────────────────────┘
```

### Crate map (23 crates)

| Crate | Purpose |
|---|---|
| `spindle-server` | Axum HTTP server: ingest, query API, health, JIT auth |
| `spindle-worker` | Async pipeline: parse → normalize → filter → SQL insert |
| `spindle-cli` | Operator CLI: query, inspect, trigger pipeline (table + JSON) |
| `spindle-config` | Layered config: defaults → TOML → `SPINDLE_*` env vars |
| `spindle-obs` | Observability: structured logging, tracing subscriber |
| `spindle-error` | Shared error type hierarchy |
| `spindle-rawarchive` | Raw payload archive: local FS + S3/MinIO backends |
| `spindle-store` | Typed PostgreSQL stores with compile-time scope enforcement |
| `spindle-pipeline` | Shared pipeline types, normalization, filtering |
| `spindle-api` | Shared API types: filters, pagination, sort, enums |
| `spindle-identity` | Identity mapping rules: OIDC claims → roles/scopes |
| `spindle-authz` | Scope, role, and authorization primitives |
| `spindle-signing` | PGP/GPG content signing |
| `spindle-compliance` | Compliance report parsing and control evaluation |
| `spindle-archive` | Archive retention and lifecycle management |
| `spindle-shutdown` | Graceful shutdown coordination |
| `spindle-migrate` | Database migration runner |
| `spindle-dex` | Dex identity provider integration + health checks |
| `spindle-saml` | SAML assertion handling |
| `spindle-bench` | Benchmarking harness |
| `spindle-dashboard` | Web UI dashboard |
| `mcp-server` | Model Context Protocol server |
| `spindle-mcp` | Spindle-specific MCP tools for AI agents |

## Quick Start

### Prerequisites

- Rust stable (`rustup toolchain install stable`)
- Docker + Docker Compose (for PostgreSQL, MinIO, Keycloak)
- Or a running PostgreSQL 15+ instance

### 1. Start infrastructure

```bash
make test-up
```

This starts PostgreSQL, MinIO (S3-compatible), and Keycloak via Docker Compose.

### 2. Run the server

```bash
# Set your ingest token
export SPINDLE_INGEST_TOKEN="my-secret-token"

# Run in dev mode (in-memory fallback if DB is down)
cargo run -p spindle-server

# Or run in production mode (exits 1 if DB unreachable)
SPINDLE_PRODUCTION=1 cargo run -p spindle-server
```

Server starts on `http://127.0.0.1:3000`.

### 3. Test the health endpoint

```bash
curl http://127.0.0.1:3000/v1/health
# {"api_version":"v1","status":"up","http_status":200,...}

curl http://127.0.0.1:3000/v1/health/metrics
# Prometheus-format metrics
```

### 4. API documentation

Visit `http://127.0.0.1:3000/docs` for Swagger UI, or `http://127.0.0.1:3000/openapi.json` for the raw spec.

### 5. CLI usage

```bash
cargo run -p spindle-cli -- --help
```

## Configuration

Configuration is layered via [Figment](https://crates.io/crates/figment):

1. **Defaults** — hardcoded sensible defaults
2. **Config file** — `~/.config/spindle/config.toml` or path in `SPINDLE_CONFIG`
3. **Environment variables** — `SPINDLE_SERVER_HOST`, `SPINDLE_DATABASE_URL`, etc.

Example config:

```toml
[server]
host = "127.0.0.1"
port = 3000

[database]
url = "postgres://spindle:spindle@localhost:5432/spindle"
pool_max = 20
pool_min = 5

[storage]
backend = "local"
bucket = "spindle-data"

[signing]
mode = "disabled"
hash_algorithm = "sha256"
```

### Key environment variables

| Variable | Default | Description |
|---|---|---|
| `SPINDLE_DATABASE_URL` | `postgres://spindle:spindle@localhost:5432/spindle` | PostgreSQL connection |
| `SPINDLE_INGEST_TOKEN` | `spindle-dev-token` | Bearer token for ingest + API |
| `SPINDLE_ARCHIVE_DIR` | `/var/lib/spindle/archive` | Raw archive root |
| `SPINDLE_PRODUCTION` | (unset) | Set to `1` for production mode (DB required) |
| `SPINDLE_LOG_LEVEL` | `operational` | Logging tier: `operational` / `diagnostic` / `debug` |
| `SPINDLE_LOG_TARGET` | `json` | `stdout` or `json` |

## Database Migrations

Migrations live in `migrations/` and are run via the `spindle-migrate` crate:

```bash
cargo run -p spindle-migrate
```

See `migrations/` for the full migration history.

## Development

See **[AGENTS.md](AGENTS.md)** for the complete engineering conventions guide — build commands, code style, security rules, migration conventions, and development workflow.

```bash
# Format + lint
cargo fmt --all
cargo clippy --all-targets -- -D warnings

# Test
cargo test --workspace
cargo audit

# Start dev server
make test-up
cargo run -p spindle-server
```

## Documentation

| Document | Description |
|---|---|
| [AGENTS.md](AGENTS.md) | Engineering conventions — build, test, code style, security |
| [AGENT-TASKS.md](AGENT-TASKS.md) | Phased get-well plan from audit findings |
| [AUDIT-REPORT.md](AUDIT-REPORT.md) | Enterprise audit report + findings |
| [BRIEF.md](BRIEF.md) | Project status and context as of 2026-08-11 |
| [PLANS.md](PLANS.md) | Detailed implementation plans |
| [docs/EXECUTION-ARCHITECTURE.md](docs/EXECUTION-ARCHITECTURE.md) | Architecture deep-dive |
| [docs/access-architecture.md](docs/access-architecture.md) | CLI, Web UI, MCP access design |
| [docs/logging-architecture.md](docs/logging-architecture.md) | Logging tiers and conventions |
| [docs/STUBS.md](docs/STUBS.md) | Stub replacement task tracking |

## License

See [LICENSE](LICENSE) for details.

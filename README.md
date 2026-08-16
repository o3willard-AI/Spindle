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

If you are a developer working on the Spindle codebase, see [Developer Quick Start](#developer-quick-start). If you are an operator deploying a pre-built binary to production, see [Operator Quick Start](#operator-quick-start).

## Developer Quick Start

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

## Operator Quick Start

> **You already have CINC Server, CINC Workstation, CINC Infra Clients, and CINC Auditor (InSpec) deployed.** Spindle ships as a pre-built binary — no Rust toolchain, no Docker, no compilation required. This guide gets you running in under 5 minutes.

### Prerequisites

Before starting, verify each component of your stack is in place:

| Component | Version | What It Does | Why Spindle Needs It |
|-----------|---------|-------------|---------------------|
| **CINC Server** | 15.x (tested 15.10.114) | Fleet server / policy management | Spindle ingests data-collector events from clients managed by CINC Server |
| **CINC Workstation** | 26.x (tested 26.2.2) | ChefDK for running Chef tools | Used to configure data-collector target on CINC Server; sets client `data_collector.server_url` and `token` |
| **CINC Infra Clients** | 19.x (tested 19.3.14) | Node configuration management (cinc-client) | Each node runs `cinc-client` with data-collector enabled — Spindle receives the run-converge payload |
| **CINC Auditor (InSpec)** | 7.x (tested 7.1.7) | Compliance scanning | Nodes run `cinc-auditor` profile scans — Spindle receives JSON compliance reports alongside converge events |
| **PostgreSQL** | 16 recommended (15 min) | Database for Spindle | Stores all normalized node/run/compliance data; Spindle runs migrations on first startup |
| **S3-compatible storage** (MinIO or AWS S3) | S3 API | Raw payload archive | Spindle archives raw data-collector + inspec JSON before parsing (write-before-parse guarantee) |
| **Ubuntu 24.04** | LTS | Host OS | Spindle server binary runs natively on Ubuntu |
| **Server: ≥4GB RAM, ≥20GB disk** | — | Host resources | Spindle server + PostgreSQL + archive metadata |
| **Spindle binary** | Latest release | Download from [releases](https://github.com/o3willard-AI/Spindle/releases) | The Spindle server binary — no build step |

### 1. Download the Spindle binary

```bash
# Download the latest release for linux-x86_64
curl -L https://github.com/o3willard-AI/Spindle/releases/latest/download/spindle-server-linux-x86_64 -o /usr/local/bin/spindle-server
chmod +x /usr/local/bin/spindle-server
```

### 2. Install database migrations

```bash
# Run once on the Spindle server — creates all tables in PostgreSQL
export SPINDLE_DATABASE_URL="postgres://spindle:spindle@YOUR-DB-HOST:5432/spindle"
spindle-server --migrate
```

### 3. Configure Spindle

Create `/etc/spindle/config.toml`:

```toml
[server]
host = "0.0.0.0"
port = 3000

[database]
url = "postgres://spindle:spindle@YOUR-DB-HOST:5432/spindle"

[storage]
backend = "s3"
bucket = "spindle-archive"
s3_endpoint = "https://minio.YOUR-DOMAIN.COM"
s3_access_key = "YOUR-ACCESS-KEY"
s3_secret_key = "YOUR-SECRET-KEY"
s3_region = "us-east-1"

[signing]
mode = "disabled"

[log]
level = "operational"
target = "json"
```

> **Full configuration reference:** see [docs/operator/quick-start.md](docs/operator/quick-start.md)

### 4. Set required environment variables

```bash
export SPINDLE_PRODUCTION=1
export SPINDLE_INGEST_TOKEN="GENERATE-A-STRONG-SECRET-HERE"
export SPINDLE_JWT_SECRET="GENERATE-A-SECOND-SECRET-HERE"
```

Generate secrets with: `openssl rand -hex 32`

### 5. Run as a systemd service

Create `/etc/systemd/system/spindle-server.service`:

```ini
[Unit]
Description=Spindle Fleet Observability Server
After=network.target postgresql.service

[Service]
Type=simple
User=spindle
EnvironmentFile=/etc/spindle/spindle.env
ExecStart=/usr/local/bin/spindle-server
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
# Write env file
cat > /etc/spindle/spindle.env <<EOF
SPINDLE_PRODUCTION=1
SPINDLE_INGEST_TOKEN=$(openssl rand -hex 32)
SPINDLE_JWT_SECRET=$(openssl rand -hex 32)
SPINDLE_DATABASE_URL=postgres://spindle:spindle@YOUR-DB-HOST:5432/spindle
EOF

# Enable and start
systemctl daemon-reload
systemctl enable spindle-server
systemctl start spindle-server
systemctl status spindle-server
```

### 6. Configure CINC Server data-collector target

In your CINC Server **Attributes → Organization** (or `client.rb` on nodes), configure the data-collector:

```ruby
# client.rb on each node
data_collector.server_url = "https://spindle.YOUR-DOMAIN.COM/ingest/events/data-collector"
data_collector.token = "YOUR_SPINDLE_INGEST_TOKEN"
data_collector.organization_names = ["your-org"]
data_collector.env_var = "SPINDLE_INGEST_TOKEN"

# Enable compliance reporting
data_collector.environment = "production"
```

### 7. Verify ingestion

```bash
# Check health
curl https://spindle.YOUR-DOMAIN.COM/v1/health

# Query nodes (should show your fleet)
curl -H "Authorization: Bearer YOUR_SPINDLE_INGEST_TOKEN" \
  https://spindle.YOUR-DOMAIN.COM/v1/nodes
```

### What happens next

1. CINC Infra Client runs → sends run-converge JSON to Spindle ingest endpoint
2. Spindle archives the raw JSON to S3/MinIO (write-before-parse guarantee)
3. Spindle worker parses, normalizes, and inserts into PostgreSQL
4. Query API is immediately available at `/v1/nodes`, `/v1/runs`, `/v1/compliance/*`

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
| [docs/operator/quick-start.md](docs/operator/quick-start.md) | Full operator deployment guide (binary install, systemd, CINC config) |
| [docs/operator/cinc-integration.md](docs/operator/cinc-integration.md) | Consolidated guide: add Spindle to an existing CINC fleet (data-collector + InSpec, verify, troubleshoot) |
| [docs/operator/backup-restore.md](docs/operator/backup-restore.md) | Backup and restore procedures |
| [docs/operator/storage-requirements.md](docs/operator/storage-requirements.md) | Storage sizing guide |
| [docs/EXECUTION-ARCHITECTURE.md](docs/EXECUTION-ARCHITECTURE.md) | Architecture deep-dive |
| [docs/access-architecture.md](docs/access-architecture.md) | CLI, Web UI, MCP access design |
| [docs/logging-architecture.md](docs/logging-architecture.md) | Logging tiers and conventions |
| [docs/STUBS.md](docs/STUBS.md) | Stub replacement task tracking |

## License

See [LICENSE](LICENSE) for details.

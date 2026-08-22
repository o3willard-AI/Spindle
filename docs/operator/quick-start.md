# Operator Quick Start — Spindle

> **Target audience:** Operators deploying a pre-built Spindle binary to production.
> **Prerequisites:** CINC Server, CINC Workstation, CINC Infra Clients + CINC Auditor (Cinc Auditor) already deployed to your fleet. PostgreSQL 16. Linux host with glibc ≥ 2.34 (Ubuntu 24.04, AlmaLinux 9, Rocky Linux 9). S3/MinIO or local disk.

This document is the full operator-focused guide. For a condensed 5-minute version, see [README.md#operator-quick-start](../../README.md#operator-quick-start).

---

## 1. Prerequisites Verification

Before installing Spindle, verify each component of your stack:

### CINC Stack

| Component | Tested Version | Verify Command |
|-----------|---------------|----------------|
| CINC Server | 15.x (tested 15.10.114) | `cinc-server-ctl status` |
| CINC Workstation | 26.x (tested 26.2.2) | `cinc --version` |
| CINC Infra Client | 19.x (tested 19.3.14) | `cinc-client --version` (on a managed node) |
| CINC Auditor (Cinc Auditor) | 7.x (tested 7.1.7) | `cinc-auditor --version` (on a managed node) |

### Infrastructure

| Component | Requirement | Notes |
|-----------|-------------|-------|
| PostgreSQL | 16 recommended (15 min) | Create a dedicated `spindle` database and user |
| Storage | S3-compatible or local disk | S3 backend recommended for multi-node deployments |
| Host OS | Linux, glibc ≥ 2.34 (Ubuntu 24.04, AlmaLinux 9, Rocky Linux 9) | Spindle binaries run natively; Ubuntu 24.04 is the primary tested target |
| Resources | ≥4GB RAM, ≥20GB disk | SSD recommended for database |

### Spindle Binaries

Download **four** binaries from the [latest GitHub release](https://github.com/o3willard-AI/Spindle/releases/latest) — `spindle-server` (HTTP API + ingest), `spindle-worker` (async pipeline: parse → normalize → insert), and `spindle-migrate` (database migration runner), and `spindle-dashboard` (web UI):

```bash
for bin in spindle-server spindle-worker spindle-migrate spindle-dashboard; do
  curl -L "https://github.com/o3willard-AI/Spindle/releases/latest/download/${bin}-linux-x86_64" \
    -o "/usr/local/bin/${bin}"
  chmod +x "/usr/local/bin/${bin}"
done
```

Verify:

```bash
spindle-server --version
spindle-worker --version   # if the release includes version output
spindle-migrate --version
```

---

## 2. Database Setup

Create the Spindle database and user in PostgreSQL:

```sql
CREATE USER spindle WITH PASSWORD 'GENERATE-A-STRONG-PASSWORD-HERE';
CREATE DATABASE spindle OWNER spindle;
GRANT ALL PRIVILEGES ON DATABASE spindle TO spindle;
-- PostgreSQL 15+ no longer grants non-owners CREATE on the public schema by
-- default; grant it explicitly so spindle-migrate can create the tables:
GRANT ALL ON SCHEMA public TO spindle;
```

Run migrations using the separate `spindle-migrate` binary (not `spindle-server`):

```bash
export SPINDLE_DATABASE_URL="postgres://spindle:YOUR-PASSWORD@YOUR-DB-HOST:5432/spindle"
spindle-migrate --migrations-dir /opt/spindle/migrations
```

`spindle-migrate` accepts `--database-url <URL>` (or reads `$SPINDLE_DATABASE_URL` / `$DATABASE_URL`) and `--migrations-dir <DIR>` (default: `./migrations`). Run `spindle-migrate --help` for full options.
The migration SQL is **not** bundled inside the `spindle-migrate` binary. You
must supply the `migrations/` directory from a source checkout (or a source
tarball). For example:

```bash
git clone https://github.com/o3willard-AI/Spindle /opt/spindle-src
spindle-migrate --migrations-dir /opt/spindle-src/migrations
```


You should see output indicating all migrations applied successfully.

---

## 3. Configuration

### Config file (primary)

Create `/etc/spindle/config.toml`:

```toml
[server]
host = "0.0.0.0"
port = 3000
# Set to true if behind a reverse proxy (TLS termination, X-Forwarded-*)
behind-proxy = true

[database]
url = "postgres://spindle:YOUR-PASSWORD@YOUR-DB-HOST:5432/spindle"
pool-max = 20
pool-min = 5

[storage]
# Options: "local", "s3"
# "local" is the simplest first deploy (no MinIO/S3 required).
# Use "s3" for multi-node deployments or when you need shared archive storage.
backend = "local"

# Local filesystem backend (used when backend = "local")
local_root = "/var/lib/spindle/archive"

# S3/MinIO backend (used when backend = "s3"; see scripts/minio-init.sh
# for bucket setup)
bucket = "spindle-archive"
endpoint = "https://minio.YOUR-DOMAIN.COM"
access-key-id = "YOUR-ACCESS-KEY"
secret-access-key = "YOUR-SECRET-KEY"
region = "us-east-1"
path-style = false

[signing]
# For production, use "aws-kms" or "pkcs11" with a configured hardware key
mode = "disabled"

[log]
level = "operational"  # operational | diagnostic | debug
target = "json"       # json | stdout
```

### Environment variables (override config)

Create `/etc/spindle/spindle.env`:

```bash
# Production mode. NOTE: SPINDLE_PRODUCTION=1 requires built-in TLS
# (SPINDLE_TLS_ENABLED=1 + cert/key below) and a JWT secret. For plain HTTP
# behind a TLS-terminating reverse proxy, leave SPINDLE_PRODUCTION unset.
SPINDLE_PRODUCTION=1

# Required when SPINDLE_PRODUCTION=1 — built-in TLS
SPINDLE_TLS_ENABLED=1
SPINDLE_TLS_CERT=/etc/spindle/tls/fullchain.pem
SPINDLE_TLS_KEY=/etc/spindle/tls/privkey.pem

# Required — bearer token for ingest + API auth
SPINDLE_INGEST_TOKEN=GENERATE-A-STRONG-SECRET

# Required — JWT signing secret (64+ hex chars)
# Must be DIFFERENT from SPINDLE_INGEST_TOKEN.
SPINDLE_JWT_SECRET=GENERATE-A-SECOND-SECRET

# Optional — Dex/OIDC is NOT required for basic ingest + query.
# SPINDLE_INGEST_TOKEN + SPINDLE_JWT_SECRET are sufficient for bearer-token
# auth. Dex provides OIDC login + JIT user provisioning as a separate optional
# step (see scripts/deploy-dex.sh).

# Database (overrides config file if set)
SPINDLE_DATABASE_URL=postgres://spindle:YOUR-PASSWORD@YOUR-DB-HOST:5432/spindle

# Archive directory (for local backend)
SPINDLE_ARCHIVE_DIR=/var/lib/spindle/archive

# Optional — config file path (defaults to /etc/spindle/config.toml)
SPINDLE_CONFIG=/etc/spindle/config.toml

# Optional — log level / target override
SPINDLE_LOG_LEVEL=operational
SPINDLE_LOG_TARGET=json
```

Generate secrets:

```bash
openssl rand -hex 32  # for SPINDLE_INGEST_TOKEN
openssl rand -hex 32  # for SPINDLE_JWT_SECRET
```

**Never commit these secrets to version control.**

---

## 4. Systemd Services

Spindle requires **two** daemons: `spindle-server` (HTTP API + ingest) and
`spindle-worker` (async pipeline: parse → normalize → insert). The worker
polls the PostgreSQL job queue for archived payloads and processes them —
without it, ingested payloads are archived but never inserted into the query
tables.

### spindle-server

Create the service unit `/etc/systemd/system/spindle-server.service`:

```ini
[Unit]
Description=Spindle Fleet Observability Server
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
Type=simple
User=spindle
Group=spindle
WorkingDirectory=/var/lib/spindle
EnvironmentFile=/etc/spindle/spindle.env
ExecStart=/usr/local/bin/spindle-server
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/spindle /etc/spindle

[Install]
WantedBy=multi-user.target
```

### spindle-worker

Create `/etc/systemd/system/spindle-worker.service`:

```ini
[Unit]
Description=Spindle Pipeline Worker
After=network-online.target postgresql.service spindle-server.service
Wants=network-online.target

[Service]
Type=simple
User=spindle
Group=spindle
WorkingDirectory=/var/lib/spindle
EnvironmentFile=/etc/spindle/spindle.env
ExecStart=/usr/local/bin/spindle-worker
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/spindle /etc/spindle

[Install]
WantedBy=multi-user.target
```

### Create system user and directories

```bash
useradd --system --no-create-home --shell /usr/sbin/nologin spindle
mkdir -p /var/lib/spindle/archive /etc/spindle
chown -R spindle:spindle /var/lib/spindle /etc/spindle
```

Enable and start:

```bash
systemctl daemon-reload
systemctl enable spindle-server spindle-worker
systemctl start spindle-server spindle-worker
systemctl status spindle-server spindle-worker
```

---

### spindle-dashboard (optional web UI)

Spindle ships an optional stateless web dashboard. It is a separate binary and
service from `spindle-server`/`spindle-worker`: it serves the UI and proxies API
calls to the Spindle REST API.

```bash
spindle-dashboard --api-url http://127.0.0.1:3000 --port 3000
```

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--api-url` | `SPINDLE_API_URL` | `http://127.0.0.1:8080` | Spindle REST API base URL the dashboard proxies to (set to your `spindle-server` URL) |
| `--port` | — | `3000` | Port the dashboard listens on |

Example unit `/etc/systemd/system/spindle-dashboard.service`:

```ini
[Unit]
Description=Spindle Web Dashboard
After=network-online.target spindle-server.service
Wants=network-online.target

[Service]
Type=simple
User=spindle
Group=spindle
Environment=SPINDLE_API_URL=http://127.0.0.1:3000
ExecStart=/usr/local/bin/spindle-dashboard --port 3000
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

---

## 5. CINC Server Configuration

### On the CINC Server

Configure the data-collector to forward node run-converge events to Spindle:

1. On your CINC Server, set the organization-level data-collector attributes
   (e.g. via `knife` or your organization attributes management tool):

```json
{
  "data_collector": {
    "server_url": "https://spindle.YOUR-DOMAIN.COM/ingest/events/data-collector",
    "token": "YOUR_SPINDLE_INGEST_TOKEN",
    "organization_names": ["your-org"]
  }
}
```

### On each managed node (client.rb)

Ensure `/etc/cinc/client.rb` includes the data-collector configuration using
**hash notation** (not dot notation — `client.rb` is Ruby and requires the
hash form for nested attributes):

```ruby
# Direct data-collector payloads to Spindle
data_collector['server_url'] = 'https://spindle.YOUR-DOMAIN.COM/ingest/events/data-collector'
data_collector['token'] = 'YOUR_SPINDLE_INGEST_TOKEN'
```

These are the only two keys required. Do not add `data_collector['environment']`
or `data_collector['organization_names']` — they are not part of the verified
CINC client configuration.

> **Auth header:** `data_collector['token']` is sent to Spindle in the
> `X-Data-Collector-Token` header (raw, no `Bearer ` prefix) — the Cinc
> data-collector hardcodes this Chef wire format. The Cinc Auditor route uses
> `Authorization: Bearer` instead. See
> [cinc-integration.md](cinc-integration.md) for the full auth table and a
> sample run-converge payload.

### Cinc Auditor compliance reporting (optional)

Cinc Auditor compliance reports are sent to Spindle via a separate systemd
timer that runs `cinc-auditor exec --reporter json` and posts the result to
`/ingest/events/auditor`. This is **not** configured in `client.rb`.

To install the auditor scan timer on each managed node:

1. Copy the auditor-scan scripts and unit files to the node:

```bash
scp -r systemd-timers/auditor-scan/ ubuntu@NODE_IP:/tmp/auditor-scan/
```

2. Install the script, service, and timer:

```bash
sudo mkdir -p /opt/spindle/scripts/auditor-scan
sudo cp /tmp/auditor-scan/auditor-scan.sh /opt/spindle/scripts/auditor-scan/
sudo cp /tmp/auditor-scan/auditor-scan.service /etc/systemd/system/
sudo cp /tmp/auditor-scan/auditor-scan.timer /etc/systemd/system/
```

3. Enable and start the timer (runs every 2 minutes):

```bash
systemctl daemon-reload
systemctl enable --now spindle-auditor-scan.timer
```

The timer executes `cinc-auditor exec <profile> --reporter json`, parses the
result, and posts compliance JSON to `POST /ingest/events/auditor` on the
Spindle server.

The auditor payload the server expects includes `node_name`, `run_id`,
`organization`, a `platform` object (with `name`), a `profiles` array (each
entry with `name`, `sha256`, `version`), and a `statistics` object. The bundled
`auditor-scan.sh` already produces this shape; for a custom scanner, see the
auditor handler in `spindle-server/src/ingest.rs` for the authoritative field
list.

### TLS and reverse proxy

Spindle listens on plain HTTP by default. There are two supported ways to put
TLS in front of Spindle:

- **Built-in TLS** (required for `SPINDLE_PRODUCTION=1`): set
  `SPINDLE_TLS_ENABLED=1` plus `SPINDLE_TLS_CERT`/`SPINDLE_TLS_KEY` (see §3).
  The server refuses to start in production mode without it.
- **Reverse proxy** (recommended, keeps the server on plain HTTP): leave
  `SPINDLE_PRODUCTION` unset, set `behind-proxy = true` in `config.toml`, and
  terminate TLS at nginx/caddy in front of Spindle.

**nginx example:**

```nginx
server {
    listen 443 ssl;
    server_name spindle.YOUR-DOMAIN.COM;

    ssl_certificate /etc/letsencrypt/live/YOUR-DOMAIN.COM/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/YOUR-DOMAIN.COM/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_buffering off;
    }
}
```

---

## 6. Verification

### Health check

```bash
curl https://spindle.YOUR-DOMAIN.COM/v1/health
```

Expected response (HTTP 200):

```json
{
  "api_version": "v1",
  "status": "up",
  "http_status": 200,
  "subsystems": [
    { "name": "database", "status": "up", "latency_ms": 1 },
    { "name": "storage", "status": "up", "latency_ms": 0 },
    { "name": "dex", "status": "up", "latency_ms": 0 }
  ],
  "ingest_lag": { "queue_depth": 0, "oldest_unprocessed_seconds": null }
}
```

### Ingest test

From a node with CINC client, run a test cinc-client:

```bash
sudo cinc-client
```

Then query Spindle:

```bash
curl -H "Authorization: Bearer YOUR_SPINDLE_INGEST_TOKEN" \
  https://spindle.YOUR-DOMAIN.COM/v1/nodes
```

You should see JSON with node inventory.

### Metrics

```bash
curl -H "Authorization: Bearer YOUR_SPINDLE_INGEST_TOKEN" \
  https://spindle.YOUR-DOMAIN.COM/v1/health/metrics
```

Prometheus-format metrics in text exposition format.

---

## 7. Common Operations

### View logs

```bash
journalctl -u spindle-server -f
```

### Restart

```bash
systemctl restart spindle-server
```

### Check migration status

```bash
SPINDLE_DATABASE_URL="postgres://..." spindle-migrate --migrations-dir /opt/spindle/migrations
```

### Upgrade

```bash
# Stop services
systemctl stop spindle-server spindle-worker

# Download new binaries
for bin in spindle-server spindle-worker spindle-migrate; do
  curl -L "https://github.com/o3willard-AI/Spindle/releases/download/v0.2.3/${bin}-linux-x86_64" \
    -o "/usr/local/bin/${bin}"
  chmod +x "/usr/local/bin/${bin}"
done

# Run migrations (if new ones exist)
SPINDLE_DATABASE_URL="postgres://..." spindle-migrate --migrations-dir /opt/spindle/migrations

# Start services
systemctl start spindle-server spindle-worker
```

---

## 8. Troubleshooting

### Spindle won't start — "FATAL: SPINDLE_DATABASE_URL must be set"

In production mode (`SPINDLE_PRODUCTION=1`), the database URL is required. Set it in your environment file or config:

```bash
export SPINDLE_DATABASE_URL="postgres://spindle:pass@host:5432/spindle"
```

### Spindle won't start — "FATAL: SPINDLE_JWT_SECRET is required"

Required in production mode. Generate and set:

```bash
export SPINDLE_JWT_SECRET=$(openssl rand -hex 32)
```

### Health check shows database "down"

- Verify PostgreSQL is running: `systemctl status postgresql`
- Verify the database URL is correct: `psql $SPINDLE_DATABASE_URL -c '\l'`
- Check firewall: Spindle server must reach the DB port (default 5432)

### Nodes not appearing after cinc-client run

- Check CINC client `client.rb` has the correct `data_collector['server_url']`
- Verify the ingest token matches (`SPINDLE_INGEST_TOKEN`)
- Check Spindle logs: `journalctl -u spindle-server -f`
- Verify network connectivity from nodes to the Spindle ingest endpoint

### Archive storage errors

- For S3 backend: verify access key, secret key, and bucket name
- For local backend: check disk space on `SPINDLE_ARCHIVE_DIR`
- Check file permissions: the `spindle` user must have write access

---

## 9. API Quick Reference

All endpoints require `Authorization: Bearer <SPINDLE_INGEST_TOKEN>`.

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/v1/health` | Health check (no auth) |
| GET | `/v1/health/metrics` | Prometheus metrics |
| GET | `/docs` | Swagger UI |
| GET | `/openapi.json` | OpenAPI spec |
| GET | `/v1/nodes` | List nodes |
| GET | `/v1/nodes/{id}` | Get node details |
| GET | `/v1/runs` | List runs |
| GET | `/v1/runs/{id}` | Get run details |
| GET | `/v1/compliance/profiles` | List compliance profiles |
| GET | `/v1/compliance/reports` | List compliance reports |
| GET | `/v1/cookbooks` | List cookbooks |
| GET | `/v1/waivers` | List waivers |
| GET | `/v1/runs/{id}/resource-events` | Resource change events for a run |
| GET | `/v1/resource-events/aggregates` | Resource change aggregates (rollup) |
| GET | `/v1/resource-events/drift` | Resource drift detection |

Query parameters: `filter[field]=value`, `sort=field:asc`, `limit=50&cursor=<base64>`, `since=<RFC3339>&until=<RFC3339>`.

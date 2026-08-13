# Operator Quick Start — Spindle

> **Target audience:** Operators deploying a pre-built Spindle binary to production.
> **Prerequisites:** CINC Server (Automate), CINC Workstation, CINC Infra Clients + CINC Inspec Clients already deployed to your fleet. PostgreSQL 16. Ubuntu 24.04. S3/MinIO or local disk.

This document is the full operator-focused guide. For a condensed 5-minute version, see [README.md#operator-quick-start](README.md#operator-quick-start).

---

## 1. Prerequisites Verification

Before installing Spindle, verify each component of your stack:

### CINC Stack

| Component | Minimum Version | Verify Command |
|-----------|---------------|----------------|
| CINC Server (Automate) | 4.x | `automatectl status` |
| CINC Workstation | 23.x | `chef-client --version` (on a workstation node) |
| CINC Infra Client | 18.x | `chef-client --version` (on a managed node) |
| CINC Inspec Client | 5.x | `inspec --version` (on a managed node) |

### Infrastructure

| Component | Requirement | Notes |
|-----------|-------------|-------|
| PostgreSQL | 16 recommended (15 min) | Create a dedicated `spindle` database and user |
| Storage | S3-compatible or local disk | S3 backend recommended for multi-node deployments |
| Host OS | Ubuntu 24.04 LTS | Spindle binary runs natively |
| Resources | ≥4GB RAM, ≥20GB disk | SSD recommended for database |

### Spindle Binary

Download from the [latest GitHub release](https://github.com/o3willard-AI/Spindle/releases/latest):

```bash
curl -L https://github.com/o3willard-AI/Spindle/releases/latest/download/spindle-server-linux-x86_64 \
  -o /usr/local/bin/spindle-server
chmod +x /usr/local/bin/spindle-server
```

Verify:

```bash
spindle-server --version
```

---

## 2. Database Setup

Create the Spindle database and user in PostgreSQL:

```sql
CREATE USER spindle WITH PASSWORD 'GENERATE-A-STRONG-PASSWORD-HERE';
CREATE DATABASE spindle OWNER spindle;
GRANT ALL PRIVILEGES ON DATABASE spindle TO spindle;
```

Run migrations (bundled with the binary):

```bash
export SPINDLE_DATABASE_URL="postgres://spindle:YOUR-PASSWORD@YOUR-DB-HOST:5432/spindle"
spindle-server --migrate
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
behind_proxy = true

[database]
url = "postgres://spindle:YOUR-PASSWORD@YOUR-DB-HOST:5432/spindle"
pool_max = 20
pool_min = 5

[storage]
# Options: "local", "s3"
backend = "s3"

# Local filesystem backend
local_root = "/var/lib/spindle/archive"

# S3/MinIO backend
bucket = "spindle-archive"
s3_endpoint = "https://minio.YOUR-DOMAIN.COM"
s3_access_key = "YOUR-ACCESS-KEY"
s3_secret_key = "YOUR-SECRET-KEY"
s3_region = "us-east-1"
s3_use_tls = true
s3_use_path_style = false

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
# Required — production mode
SPINDLE_PRODUCTION=1

# Required — bearer token for ingest + API auth
SPINDLE_INGEST_TOKEN=GENERATE-A-STRONG-SECRET

# Required — JWT signing secret (64+ hex chars)
SPINDLE_JWT_SECRET=GENERATE-A-SECOND-SECRET

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

## 4. Systemd Service

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

Create the system user and directories:

```bash
useradd --system --no-create-home --shell /usr/sbin/nologin spindle
mkdir -p /var/lib/spindle/archive /etc/spindle
chown -R spindle:spindle /var/lib/spindle /etc/spindle
```

Enable and start:

```bash
systemctl daemon-reload
systemctl enable spindle-server
systemctl start spindle-server
systemctl status spindle-server
```

---

## 5. CINC Server Configuration

### On the CINC Server (Automate)

Configure the data-collector to forward node run-converge events to Spindle:

1. Navigate to **Admin → Attributes → Organization** in the Automate UI.
2. Add a custom attribute:

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

Ensure `client.rb` includes:

```ruby
data_collector.server_url = "https://spindle.YOUR-DOMAIN.COM/ingest/events/data-collector"
data_collector.token = "YOUR_SPINDLE_INGEST_TOKEN"
data_collector.organization_names = ["your-org"]

# Enable InSpec compliance reporting
data_collector.environment = "production"
```

### TLS and reverse proxy

Spindle listens on plain HTTP by default. For production, place it behind a TLS-terminating reverse proxy:

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
  "checks": {
    "database": { "status": "up", "latency_ms": 2 },
    "archive": { "status": "up" },
    "signing": { "status": "up" }
  }
}
```

### Ingest test

From a node with CINC client, run a test chef-client:

```bash
sudo chef-client
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
SPINDLE_DATABASE_URL="postgres://..." spindle-server --migrate --dry-run
```

### Upgrade

```bash
# Stop service
systemctl stop spindle-server

# Download new binary
curl -L https://github.com/o3willard-AI/Spindle/releases/download/v0.2.0/spindle-server-linux-x86_64 \
  -o /usr/local/bin/spindle-server
chmod +x /usr/local/bin/spindle-server

# Run migrations (if new ones exist)
SPINDLE_DATABASE_URL="postgres://..." spindle-server --migrate

# Start service
systemctl start spindle-server
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

### Nodes not appearing after chef-client run

- Check CINC client `client.rb` has the correct `data_collector.server_url`
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
| GET | `/v1/resource-events` | Resource change events |

Query parameters: `filter[field:op]=value`, `sort=field:asc`, `page=1&per_page=50`, `since=<RFC3339>&until=<RFC3339>`.

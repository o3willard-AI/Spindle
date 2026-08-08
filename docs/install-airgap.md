# Spindle Air-Gap Installation Guide

This guide describes how to install and run Spindle in an air-gap environment
(no internet access) using a pre-built bundle.

## Prerequisites

| Requirement | Minimum Version |
|---|---|
| Linux OS | Debian 12 / RHEL 9 / Ubuntu 22.04+ |
| Root access | For system installation (or use `spindle-install.sh`) |
| Docker (optional) | 24.0+ if using container mode |
| 4 GB RAM | 8 GB recommended |
| 20 GB disk | For data + container images |

## Bundle Contents

`spindle-bundle.tar.gz` contains:

| Path | Description |
|---|---|
| `bin/spindle-server` | HTTP API + ingest binary (statically linked, musl) |
| `bin/spindle-worker` | Queue consumer + rollups binary (statically linked, musl) |
| `bin/spindle` | CLI binary (statically linked, musl) |
| `migrations/` | SQL migration files (up.sql) |
| `spindle.toml` | Shared configuration template |
| `docker-compose.yml` | Docker Compose for container deployment |
| `docker-images.tar` | Pre-saved Docker images (postgres, minio, spindle) |
| `spindle-install.sh` | Installation script |

## No Phone-Home Guarantee

Spindle contains **zero** outbound connection attempts for:

- ❌ License checking
- ❌ Telemetry reporting
- ❌ Update checking
- ❌ Analytics collection
- ❌ Crash reporting

All functionality is self-contained. No DNS lookups or HTTPS calls are made
at startup or runtime unless explicitly configured (e.g., OIDC provider URL).

## Installation

### Step 1: Transfer the bundle

Copy `spindle-bundle.tar.gz` to the target air-gap machine:

```bash
scp spindle-bundle.tar.gz user@airgap-host:/tmp/
```

### Step 2: Run the installer

The installer handles everything: user creation, binary placement, config setup, Docker images, and service files. It requires root privileges and performs no network operations.

```bash
cd /tmp
sudo /tmp/spindle-install.sh --bundle /tmp/spindle-bundle.tar.gz
```

Or simply run from the current directory if the bundle is present:

```bash
sudo ./spindle-install.sh
```

The installer will:

1. Create a `spindle` system user
2. Install binaries to `/opt/spindle/bin/`
3. Install config to `/etc/spindle/spindle.toml`
4. Install migrations to `/opt/spindle/migrations/`
5. Load Docker images (if Docker is available)
6. Install docker-compose.yml to `/etc/spindle/`

See [Troubleshooting](#troubleshooting) for common issues.

### Step 3: Configure

Edit the config file:

```bash
sudo vi /etc/spindle/spindle.toml
```

Key settings to update:

- `[database]` — PostgreSQL connection string (`url`, `pool_max`, `pool_min`)
- `[storage]` — S3/MinIO bucket settings (`backend`, `bucket`, `endpoint`)
- `[server]` — Bind address and port (`host`, `port`)
- `[profiles.<name>]` — CLI profile URLs

### Step 4: Start services (Docker mode)

If Docker is available:

```bash
cd /etc/spindle
docker-compose -f docker-compose.yml up -d
```

Wait for services to be healthy:

```bash
docker-compose -f docker-compose.yml ps
```

All services should show `Up` status.

### Step 5: Start standalone (non-Docker mode)

If Docker is not available (typical in air-gap environments), start binaries directly:

```bash
# Start server
sudo -u spindle /opt/spindle/bin/spindle-server \
    --config /etc/spindle/spindle.toml &

# Start worker (separate terminal or service unit)
sudo -u spindle /opt/spindle/bin/spindle-worker \
    --config /etc/spindle/spindle.toml &
```

## Verification

### Health check

```bash
curl http://localhost:3000/health
```

Expected response:

```json
{"status":"healthy","timestamp":"...","uptime_seconds":...,"subsystems":{
  "database":{"status":"up"},
  "queue":{"status":"up"},
  "storage":{"status":"up"}}}
```

Exit code 0 means the server process is running; health is determined by the JSON body.

### CLI health

```bash
/opt/spindle/bin/spindle health --server http://localhost:3000
```

Exit code 0 = healthy, exit code 3 = unhealthy (as documented by `spindle health`).

### Ingest test

```bash
curl -X POST http://localhost:3000/ingest/events/data-collector \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -d '{"type":"run_converge","node_name":"test-node","run_id":"test-001","status":"success"}'
```

Expected: HTTP 202 with receipt token and archive key.

### Query verification

Query endpoints depend on pipeline processing (worker may need to have ingested data first).
See `docs/operator/backup-restore.md` for migration and restore procedures.

## Firewall Audit

To verify no outbound connections are made during normal operation:

```bash
# Monitor for any outbound connections during startup
sudo tcpdump -i any -n 'not host 127.0.0.1' &
sudo systemctl restart spindle-server
# Press Ctrl+C after services start
# No outbound packets should appear unless you configured an external OIDC provider
```

## Updating the Bundle

To create a new air-gap bundle (run from a machine with internet access):

```bash
# 1. Build static binaries
cargo build --workspace --target x86_64-unknown-linux-musl --release

# 2. Save Docker images (if using container mode)
docker save postgres:15-alpine minio/minio:latest spindle:latest -o bundle/docker-images.tar

# 3. Package everything
cd bundle/
tar czf ../spindle-bundle.tar.gz .

# 4. Transfer to air-gap machine
scp ../spindle-bundle.tar.gz user@airgap-host:/tmp/
```

## Troubleshooting

### "permission denied" on binaries

The installer sets permissions correctly. If binaries are still not executable:

```bash
sudo chmod +x /opt/spindle/bin/*
```

### Port 3000 already in use

```bash
sudo lsof -i :3000
# Edit config to change the port
```

### Docker images not loading

```bash
docker load -i /etc/spindle/docker-images.tar
```

### Config validation fails

```bash
/opt/spindle/bin/spindle-server --config /etc/spindle/spindle.toml --validate-config
```

This will print specific field errors.

### Installer reports missing dependencies

Run the same dependencies manually first (in a connected environment, or bring them into the air-gap beforehand):

```bash
apt-get install -y postgresql-client rclone aws-cli
```

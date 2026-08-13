# Adding Spindle to an Existing CINC Fleet

This guide consolidates the "add Spindle to an existing CINC fleet" instructions that
were previously spread across the operator quick-start, the Phase 2 integration plan,
the README, and the InSpec/Cinc bridge traces. It covers pointing your CINC
Infra Clients (and InSpec/Cinc Auditor clients) at a Spindle server so run-converge
and compliance payloads are archived and normalized — no Spindle code changes required.

---

## 2. Architecture

```
CINC Client (Infra 18.x / InSpec 5.x)
    │
    │ POST /ingest/events/data-collector   (run-converge JSON)
    │ POST /ingest/events/inspec           (compliance report JSON)
    │
    ▼
Spindle Server (:3000 / https://spindle.YOUR-DOMAIN.COM)
    ├── bearer-token auth (SPINDLE_INGEST_TOKEN)
    ├── raw archive → local FS / S3 / MinIO  (write-before-parse guarantee)
    ├── job queue → PostgreSQL ingest_queue
    └── idempotency → PostgreSQL
        │
        ▼
spindle-worker
    ├── parse → normalize → filter → SQL insert
    └── exposes query API: /v1/nodes, /v1/runs, /v1/compliance/*, ...
```

The data-collector and InSpec endpoints share the same `SPINDLE_INGEST_TOKEN`
authentication. See [docs/INTEGRATION.md](../INTEGRATION.md) for the full data
population plan and [docs/EXECUTION-ARCHITECTURE.md](../EXECUTION-ARCHITECTURE.md)
for query-API scope/RBAC details.

---

## 3. Prerequisites

| Component | Minimum Version | Verify Command |
|-----------|----------------|----------------|
| CINC Server (Automate) | 4.x | `automatectl status` |
| CINC Workstation | 23.x | `chef-client --version` (on a workstation node) |
| CINC Infra Client | 18.x | `chef-client --version` (on a managed node) |
| CINC Inspec Client | 5.x | `inspec --version` (on a managed node) |

Infrastructure prerequisites are documented in [quick-start.md](quick-start.md)
§1.2: PostgreSQL 16, S3/MinIO or local disk, Ubuntu 24.04, ≥4 GB RAM / ≥20 GB disk.

---

## 4. Point CINC clients at Spindle

### On the CINC Server (Automate)

Forward all managed nodes' data-collector events to Spindle centrally via
**Admin → Attributes → Organization**:

```json
{
  "data_collector": {
    "server_url": "https://spindle.YOUR-DOMAIN.COM/ingest/events/data-collector",
    "token": "YOUR_SPINDLE_INGEST_TOKEN",
    "organization_names": ["your-org"]
  }
}
```

### On each managed node (`client.rb`)

Add to `/etc/cinc/client.rb` (or `/etc/chef/client.rb` in upstream Chef
notation, which uses the dot-syntax `data_collector.server_url` shown in
[README.md#operator-quick-start](../../README.md#operator-quick-start)
§6). The hash-syntax form below is what the QA fleet uses in
[docs/INTEGRATION.md](../INTEGRATION.md) Step 3:

```ruby
# Direct data-collector payloads to Spindle
data_collector['server_url'] = 'https://spindle.YOUR-DOMAIN.COM/ingest/events/data-collector'
data_collector['token'] = 'YOUR_SPINDLE_INGEST_TOKEN'
data_collector['organization_names'] = ['your-org']

# Forward InSpec/Cinc Auditor compliance reports to Spindle
# (same token; Spindle also exposes POST /ingest/events/inspec)
data_collector.environment = 'production'
```

> **Token:** Set `YOUR_SPINDLE_INGEST_TOKEN` to the value of the
> `SPINDLE_INGEST_TOKEN` environment variable configured on the Spindle server
> (see [quick-start.md](quick-start.md) §3). Generate with
> `openssl rand -hex 32`.

> **TLS / reverse proxy:** Spindle listens on plain HTTP by default. For
> production, terminate TLS at a reverse proxy in front of Spindle (nginx
> example in [quick-start.md](quick-start.md) §5).

---

## 5. Verify ingestion

After a node runs a converge, confirm Spindle received and processed the payload:

```bash
# 1. Health check (no auth required; HTTP 200 = up)
curl https://spindle.YOUR-DOMAIN.COM/v1/health

# Expected:
# {"api_version":"v1","status":"up","http_status":200,...}

# 2. List nodes (requires the ingest token as a bearer token)
curl -H "Authorization: Bearer YOUR_SPINDLE_INGEST_TOKEN" \
  https://spindle.YOUR-DOMAIN.COM/v1/nodes

# 3. (Optional) Prometheus metrics
curl -H "Authorization: Bearer YOUR_SPINDLE_INGEST_TOKEN" \
  https://spindle.YOUR-DOMAIN.COM/v1/health/metrics
```

If nodes don't appear immediately, the worker processes the archived payload
asynchronously (archive → queue → pipeline). Wait a few seconds, then re-query
`/v1/nodes`. You can watch the queue in the server logs:
`journalctl -u spindle-server -f` (see [quick-start.md](quick-start.md) §7).

---

## 6. Troubleshooting

### Client not reporting to Spindle

- Verify `data_collector['server_url']` / `data_collector.server_url` points to
  `https://spindle.YOUR-DOMAIN.COM/ingest/events/data-collector` (not `/ingest`
  alone, and not the Cinc Server URL).
- Confirm the ingest token in `client.rb` exactly matches
  `SPINDLE_INGEST_TOKEN` on the Spindle server.
- Check server-side logs: `journalctl -u spindle-server -f`.
- Verify network reachability from node to the Spindle endpoint
  (the Cinc Server's own data-collector endpoint returns 405 in standalone Cinc —
  Spindle's ingest path is the live one; see [fleet-02-03-bridge.md](../evidence/fleet-02-03-bridge.md)).

### 401 Unauthorized on ingest

- The `SPINDLE_INGEST_TOKEN` value must be identical on the server and in
  `client.rb`. Regenerate with `openssl rand -hex 32` if unsure, then update
  both sides.
- In production mode (`SPINDLE_PRODUCTION=1`) the token is required at startup;
  see [quick-start.md](quick-start.md) §3 and §8.

### 429 Too Many Requests (rate limited)

Spindle enforces an ingest rate limit; clients exceeding it receive `429` with
a `Retry-After` header. This is expected behavior under burst load, not an error.
If you see sustained 429s across the fleet:

- Confirm the worker is keeping up — check `/v1/health/metrics` for
  `spindle_ingest_queue_depth` and `queue_saturation`.
- Stagger client converges (e.g. run every 30 min per node, offset by host) to
  avoid synchronized bursts. See [BENCHMARKS.md](../../BENCHMARKS.md) for the
  rate-limiting acceptance criteria.

### Archive not written (raw payload missing)

- For the **local** storage backend, verify the `spindle` system user has write
  access to `SPINDLE_ARCHIVE_DIR` (`/var/lib/spindle/archive` by default):
  `chown -R spindle:spindle /var/lib/spindle/archive`.
- For the **S3/MinIO** backend, confirm the bucket name, access key, secret key,
  and endpoint in `/etc/spindle/config.toml` — a misconfigured secret causes the
  worker to fail archiving (and therefore fail parsing).
- Check disk space: `df -h /var/lib/spindle/archive`.

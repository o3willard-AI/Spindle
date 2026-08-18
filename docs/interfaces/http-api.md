# Spindle HTTP API Reference

The Spindle HTTP server (`spindle-server`) exposes a REST API on the configured
host/port (default `127.0.0.1:3000`). All endpoints except `/health`, `/ready`,
`/metrics`, `/docs`, and `/openapi.json` require authentication.

## Authentication

All protected endpoints accept a Bearer token in the `Authorization` header:

```
Authorization: Bearer <token>
```

The token is validated by the `require_jwt_role` middleware, which:

1. **JWT path** — decodes an HS256 access JWT (secret from `SPINDLE_JWT_SECRET`),
   extracts the role from the `scope` claim, and injects it into the
   `X-User-Role` header. Roles: `admin` > `token-admin` > `compliance-auditor`
   > `ingest` > `viewer`.
2. **Static token fallback** — if the token is not a JWT but matches
   `SPINDLE_INGEST_TOKEN` (default `spindle-dev-token`), the caller gets
   `viewer` role.
3. **No token / invalid** → `401 Unauthorized`.

## OpenAPI / Swagger UI

- **Interactive docs**: `GET /docs` — Swagger UI rendered in-browser.
- **OpenAPI spec**: `GET /openapi.json` — machine-readable OpenAPI 3.0 JSON,
  auto-generated from `#[utoipa::path]` attributes on handlers and
  `#[derive(ToSchema)]` on request/response types.

## Response Envelope

All API responses follow a standard envelope:

```json
{
  "api_version": "v1",
  "request_id": "uuid-string",
  "data": [ ... ],
  "pagination": { "limit": 50, "offset": 0, "has_more": false }
}
```

Error responses:

```json
{
  "api_version": "v1",
  "error": {
    "code": "not_found",
    "message": "human-readable message"
  }
}
```

---

## 1. Health & Readiness

**Unauthenticated** — no bearer token required.

### GET /health

Returns aggregate subsystem health (database, storage, Dex/IdP).

```bash
curl -s http://127.0.0.1:3000/health | jq .
```

**Response** (`200`):
```json
{
  "status": "healthy",
  "version": "0.2.0",
  "subsystems": [
    { "name": "database", "status": "healthy", "latency_ms": 3 },
    { "name": "storage", "status": "healthy", "latency_ms": 1 },
    { "name": "dex", "status": "healthy", "latency_ms": 12 }
  ]
}
```

### GET /ready

Lightweight readiness probe — returns `200` if the server is accepting traffic.

```bash
curl -s http://127.0.0.1:3000/ready
```

### GET /metrics

Prometheus-format metrics (see [metrics.md](metrics.md) for the full catalog).

```bash
curl -s http://127.0.0.1:3000/metrics | head -20
```

---

## 2. Ingest Endpoints

**Auth**: Bearer token (ingest token or JWT). Accepts raw Cinc and Cinc Auditor event payloads.

### POST /ingest/events/data-collector

Receives Cinc Client data-collector events. The payload is archived to
the raw archive and a receipt token is returned.

```bash
curl -s -X POST http://127.0.0.1:3000/ingest/events/data-collector \
  -H 'Authorization: Bearer spindle-dev-token' \
  -H 'Content-Type: application/json' \
  -d '{
    "run_id": "run-abc-123",
    "node_name": "web-server-01",
    "chef_version": "18.0.0",
    "status": "success",
    "start_time": "2026-08-13T10:00:00Z",
    "end_time": "2026-08-13T10:05:30Z"
  }' | jq .
```

**Response** (`202 Accepted`):
```json
{
  "api_version": "v1",
  "receipt_token": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "archive_key": "2026-08-13/sha256-abc123...def789.json.gz"
}
```

**Errors**: `401` (no/invalid token), `409` (duplicate run_id), `413` (payload too large).

### POST /ingest/events/auditor

Receives Cinc Auditor compliance report payloads.

```bash
curl -s -X POST http://127.0.0.1:3000/ingest/events/auditor \
  -H 'Authorization: Bearer spindle-dev-token' \
  -H 'Content-Type: application/json' \
  -d '{
    "profiles": [
      {
        "name": "cis-baseline",
        "version": "1.0.0",
        "controls": [
          {
            "id": "cis-1.1",
            "status": "passed",
            "results": []
          }
        ]
      }
    ],
    "platform": { "name": "ubuntu", "release": "22.04" }
  }' | jq .
```

**Response** (`202 Accepted`): Same envelope as data-collector with `receipt_token` and `archive_key`.

---

## 3. Nodes

**Auth**: Bearer token (JWT or static). Role: `viewer` minimum.

### GET /v1/nodes

Lists fleet nodes with optional filters and pagination.

```bash
curl -s http://127.0.0.1:3000/v1/nodes \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

With filters:
```bash
curl -s 'http://127.0.0.1:3000/v1/nodes?platform=ubuntu&status=compliant&limit=10' \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

**Query params**: `limit` (default 50), `platform`, `status`, `search`, `offset`.

**Response** (`200`):
```json
{
  "api_version": "v1",
  "request_id": "req-uuid",
  "data": [
    {
      "id": "3f9f50a9-54f7-5b20-909c-c6eb39dc7ba9",
      "name": "web-server-01",
      "platform": "ubuntu",
      "platform_version": "22.04",
      "status": "compliant",
      "chef_environment": "production",
      "project_id": "acme",
      "last_seen": "2026-08-13T10:05:30Z",
      "first_seen": "2026-08-01T08:00:00Z"
    }
  ],
  "pagination": { "limit": 50, "offset": 0, "has_more": false }
}
```

### GET /v1/nodes/:id

Returns detailed information for a single node (by UUID).

```bash
curl -s http://127.0.0.1:3000/v1/nodes/3f9f50a9-54f7-5b20-909c-c6eb39dc7ba9 \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

**Response** (`200`):
```json
{
  "api_version": "v1",
  "request_id": "req-uuid",
  "data": {
    "id": "3f9f50a9-54f7-5b20-909c-c6eb39dc7ba9",
    "name": "web-server-01",
    "platform": "ubuntu",
    "platform_version": "22.04",
    "status": "compliant",
    "chef_environment": "production",
    "policy_group": "web",
    "policy_name": "web_server",
    "project_id": "acme",
    "run_list": [],
    "attributes": {},
    "first_seen": "2026-08-01T08:00:00Z",
    "last_seen": "2026-08-13T10:05:30Z"
  }
}
```

**Errors**: `404` (node not found), `403` (scope denied).

---

## 4. Runs

**Auth**: Bearer token. Role: `viewer` minimum.

### GET /v1/runs

Lists run history with optional node/time filters.

```bash
curl -s 'http://127.0.0.1:3000/v1/runs?node_id=3f9f50a9-54f7-5b20-909c-c6eb39dc7ba9&limit=20' \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

**Query params**: `limit`, `offset`, `node_id` (UUID), `start_time`, `end_time`.

**Response** (`200`):
```json
{
  "api_version": "v1",
  "request_id": "req-uuid",
  "data": [
    {
      "id": "uuid-of-run",
      "node_id": "3f9f50a9-54f7-5b20-909c-c6eb39dc7ba9",
      "run_id": "run-abc-123",
      "status": "success",
      "start_time": "2026-08-13T10:00:00Z",
      "end_time": "2026-08-13T10:05:30Z",
      "chef_version": "18.0.0"
    }
  ],
  "pagination": { "limit": 20, "offset": 0, "has_more": false }
}
```

### GET /v1/runs/:id

Returns details for a single run (by DB row UUID, NOT the Cinc run_id).

```bash
curl -s http://127.0.0.1:3000/v1/runs/uuid-of-run \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

**Response** (`200`):
```json
{
  "api_version": "v1",
  "request_id": "req-uuid",
  "data": {
    "id": "uuid-of-run",
    "node_id": "3f9f50a9-...",
    "run_id": "run-abc-123",
    "status": "success",
    "start_time": "2026-08-13T10:00:00Z",
    "end_time": "2026-08-13T10:05:30Z",
    "chef_version": "18.0.0",
    "cookbook_set": {},
    "resource_events_count": 42
  }
}
```

**Errors**: `404` (run not found). Note: `:id` expects the DB row UUID
(returned in the `id` field of the list), not the Cinc `run_id` string.

### GET /v1/runs/:id/events

Returns resource events for a specific run.

```bash
curl -s http://127.0.0.1:3000/v1/runs/uuid-of-run/events \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

---

## 5. Resource Events

**Auth**: Bearer token. Role: `viewer` minimum.

### GET /v1/resource-events/aggregates

Returns aggregated resource event statistics grouped by cookbook, resource type,
platform, and time window.

```bash
curl -s 'http://127.0.0.1:3000/v1/resource-events/aggregates?group_by=cookbook_name&window=24h' \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

**Query params**: `group_by` (`cookbook_name`, `resource_type`, `platform`),
`window` (e.g. `1h`, `24h`, `7d`).

**Response** (`200`):
```json
{
  "api_version": "v1",
  "request_id": "req-uuid",
  "data": [
    {
      "hour": "2026-08-13T10:00:00Z",
      "cookbook_name": "apache2",
      "cookbook_version": "8.1.0",
      "resource_type": "service",
      "platform": "ubuntu",
      "count": 15,
      "sum_duration_ms": 4200,
      "avg_duration_ms": 280,
      "p50_ms": 250,
      "p95_ms": 450,
      "p99_ms": 500,
      "max_ms": 600
    }
  ]
}
```

### GET /v1/resource-events/drift

Returns resource drift detection results — identifies resources that changed
significantly across runs.

```bash
curl -s 'http://127.0.0.1:3000/v1/resource-events/drift?window=24h&threshold=5' \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

**Query params**: `window`, `threshold` (minimum change count to flag drift),
`node` (filter by node name).

**Response** (`200`):
```json
{
  "api_version": "v1",
  "request_id": "req-uuid",
  "data": [
    {
      "resource_id": "template[/etc/apache2/apache2.conf]",
      "resource_type": "template",
      "cookbook_name": "apache2",
      "platform": "ubuntu",
      "last_value": "...",
      "previous_value": "...",
      "changed_count": 7
    }
  ]
}
```

---

## 6. Compliance

**Auth**: Bearer token. Role: `viewer` minimum. **DB-backed only** — routes are
not mounted when no Postgres pool is available.

### GET /v1/compliance/reports

Lists compliance reports (Cinc Auditor scan results).

```bash
curl -s 'http://127.0.0.1:3000/v1/compliance/reports?limit=20' \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

**Query params**: `limit`, `offset`, `node` (filter by node name),
`profile` (filter by profile name).

**Response** (`200`):
```json
{
  "api_version": "v1",
  "request_id": "req-uuid",
  "data": [
    {
      "id": "report-uuid",
      "node_name": "web-server-01",
      "profile_name": "cis-baseline",
      "profile_version": "1.0.0",
      "status": "failed",
      "controls_total": 120,
      "controls_passed": 115,
      "controls_failed": 5,
      "controls_skipped": 0,
      "timestamp": "2026-08-13T10:00:00Z"
    }
  ],
  "pagination": { "limit": 20, "offset": 0, "has_more": false }
}
```

### GET /v1/compliance/reports/:id

Returns a single compliance report with full control results.

```bash
curl -s http://127.0.0.1:3000/v1/compliance/reports/report-uuid \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

**Response** (`200`):
```json
{
  "api_version": "v1",
  "request_id": "req-uuid",
  "data": {
    "id": "report-uuid",
    "node_name": "web-server-01",
    "profile_name": "cis-baseline",
    "status": "failed",
    "controls_total": 120,
    "controls_passed": 115,
    "controls_failed": 5,
    "controls_skipped": 0,
    "timestamp": "2026-08-13T10:00:00Z"
  }
}
```

### GET /v1/compliance/profiles

Lists all compliance profiles that have been observed.

### GET /v1/compliance/nodes/:node/status

Returns the latest compliance status for a specific node.

---

## 7. Cookbooks

**Auth**: Bearer token. Role: `viewer` minimum.

### GET /v1/cookbooks

Lists cookbook inventory (in-memory store, seeded from observed runs).

```bash
curl -s http://127.0.0.1:3000/v1/cookbooks \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

**Response** (`200`):
```json
{
  "api_version": "v1",
  "request_id": "req-uuid",
  "data": [
    {
      "name": "apache2",
      "versions": [
        { "version": "8.1.0", "nodes_count": 5 },
        { "version": "8.0.2", "nodes_count": 2 }
      ]
    }
  ]
}
```

---

## 8. Waivers

**Auth**: Bearer token. Role: `viewer` for read, `admin` for write.

### GET /v1/waivers

Lists active compliance waivers (non-expired only).

```bash
curl -s http://127.0.0.1:3000/v1/waivers \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

**Response** (`200`):
```json
{
  "api_version": "v1",
  "request_id": "req-uuid",
  "data": [
    {
      "id": "waiver-uuid",
      "control_id": "cis-1.1",
      "profile_id": "cis-baseline",
      "scope": "node",
      "scope_value": "web-server-01",
      "justification": "Compensating control in place",
      "approver": "security-team",
      "start_date": "2026-08-01T00:00:00Z",
      "expiry_date": "2026-09-01T00:00:00Z",
      "is_expired": false
    }
  ]
}
```

### POST /v1/waivers

Creates a new compliance waiver. Requires `admin` role.

```bash
curl -s -X POST http://127.0.0.1:3000/v1/waivers \
  -H 'Authorization: Bearer spindle-dev-token' \
  -H 'Content-Type: application/json' \
  -d '{
    "control_id": "cis-1.1",
    "profile_id": "cis-baseline",
    "scope": "node",
    "scope_value": "web-server-01",
    "justification": "Compensating control in place",
    "approver": "security-team",
    "start_date": "2026-08-13",
    "expiry_date": "2026-09-13"
  }' | jq .
```

**Response** (`200`):
```json
{
  "api_version": "v1",
  "request_id": "req-uuid",
  "data": {
    "id": "new-waiver-uuid",
    "control_id": "cis-1.1",
    "profile_id": "cis-baseline",
    "scope": "node",
    "scope_value": "web-server-01",
    "justification": "Compensating control in place",
    "approver": "security-team",
    "start_date": "2026-08-13T00:00:00Z",
    "expiry_date": "2026-09-13T00:00:00Z",
    "is_expired": false
  }
}
```

### PUT /v1/waivers/:id

Updates an existing waiver (e.g. extend expiry date). Requires `admin` role.

```bash
curl -s -X PUT http://127.0.0.1:3000/v1/waivers/waiver-uuid \
  -H 'Authorization: Bearer spindle-dev-token' \
  -H 'Content-Type: application/json' \
  -d '{
    "expiry_date": "2026-10-13"
  }' | jq .
```

### DELETE /v1/waivers/:id

Revokes (deletes) a waiver. Requires `admin` role.

```bash
curl -s -X DELETE http://127.0.0.1:3000/v1/waivers/waiver-uuid \
  -H 'Authorization: Bearer spindle-dev-token' | jq .
```

**Response** (`200`):
```json
{
  "api_version": "v1",
  "request_id": "req-uuid",
  "data": { "deleted": true }
}
```

---

## 9. Admin — Dead Letter Queue

**Auth**: Bearer token. Role: `admin` required. **DB-backed only**.

### GET /v1/admin/dead-letter

Lists pipeline dead-letter entries — jobs that permanently failed processing.

```bash
curl -s 'http://127.0.0.1:3000/v1/admin/dead-letter?limit=20&offset=0' \
  -H 'Authorization: Bearer <admin-jwt>' | jq .
```

**Query params**: `limit` (default 50, max 200), `offset` (default 0).

**Response** (`200`):
```json
{
  "api_version": "v1",
  "items": [
    {
      "id": "dlq-uuid",
      "archive_reference": "2026-08-13/sha256-abc...json.gz",
      "error_message": "JSON parse error: missing 'run_id' field",
      "error_type": "ParseError",
      "retry_count": 3,
      "payload_type": "data-collector",
      "node_name": "web-server-01",
      "run_id": "run-abc-123",
      "created_at": "2026-08-13T10:06:00Z",
      "updated_at": "2026-08-13T10:08:00Z"
    }
  ],
  "total": 1,
  "limit": 20,
  "offset": 0,
  "has_more": false
}
```

**Errors**: `403` (non-admin role), `500` (DB query failure).

---

## 10. Authentication — JIT Login

**Unauthenticated** — the login endpoint is public.

### GET /v1/auth/login

Triggers JIT (Just-In-Time) provisioning. When a caller identifies with a
connector (oidc/saml/ldap) and subject, Spindle provisions the user into
PostgreSQL, evaluates mapping rules to assign roles, and issues session JWTs.

**DB-backed only** — not mounted when no Postgres pool is available.

```bash
curl -s 'http://127.0.0.1:3000/v1/auth/login?connector=oidc&subject=user@example.com&groups=engineers' | jq .
```

**Query params**: `connector` (`oidc`, `saml`, `ldap`, `local`), `subject`
(unique user identifier), `groups` (comma-separated group memberships).

**Response** (`200`):
```json
{
  "access_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "refresh_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "token_type": "Bearer",
  "expires_in": 3600
}
```

**Errors**: `400` (invalid connector), `500` (DB/mapping failure).

---

## 11. Local Accounts

**Unauthenticated** — login endpoints are public. Uses in-memory store.

### POST /v1/auth/local/login

Authenticates a local (username/password) account.

```bash
curl -s -X POST http://127.0.0.1:3000/v1/auth/local/login \
  -H 'Content-Type: application/json' \
  -d '{
    "username": "admin",
    "password": "changeme"
  }' | jq .
```

**Response** (`200`):
```json
{
  "access_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "refresh_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "token_type": "Bearer",
  "expires_in": 3600
}
```

**Errors**: `401` (wrong password), `403` (account locked), `429` (rate limited).

### POST /v1/auth/local/register

Registers a new local account. First-run creates a bootstrap admin from env
vars (`SPINDLE_BOOTSTRAP_ADMIN_USER`, `SPINDLE_BOOTSTRAP_ADMIN_PASSWORD`).

```bash
curl -s -X POST http://127.0.0.1:3000/v1/auth/local/register \
  -H 'Content-Type: application/json' \
  -d '{
    "username": "operator1",
    "password": "secure-password",
    "roles": ["viewer"]
  }' | jq .
```

### GET /v1/auth/local/audit

Returns local account audit log entries. Requires `admin` role.

```bash
curl -s http://127.0.0.1:3000/v1/auth/local/audit \
  -H 'Authorization: Bearer <admin-jwt>' | jq .
```

---

## 12. Backup & Restore

Backup and restore are **script-based**, not HTTP routes. See
[docs/operator/backup-restore.md](../operator/backup-restore.md) for the full
procedure.

Three scripts handle backup:
- `scripts/backup-database.sh` — `pg_dump` + WAL archiving
- `scripts/backup-archive.sh` — `tar`/`rsync` of raw archive
- `scripts/backup-manifests.sh` — backup of signing manifests

Restore:
- `scripts/restore-spindle.sh` — restores database + archive + manifests

---

## 13. Sessions & Tokens

Sessions and tokens are **service-layer only** — no direct HTTP routes are
registered. They are used internally by the JIT and local auth handlers:

- **Sessions** (`sessions.rs`): `SessionManager` creates/stores sessions, issues
  HS256 JWTs (`access_token` + `refresh_token`), and validates them via
  `require_jwt_role` middleware.
- **Tokens** (`tokens.rs`): `TokenManager` supports API token CRUD, rotation,
  revocation, idle detection, and audit logging. Called by admin handlers
  (not yet exposed as HTTP routes).

---

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `SPINDLE_CONFIG` | `~/.config/spindle/config.toml` | Config file path |
| `SPINDLE_DATABASE_URL` | `postgres://spindle:CHANGE_ME@localhost:5432/spindle` | PostgreSQL connection |
| `SPINDLE_INGEST_TOKEN` | `spindle-dev-token` | Bearer token for ingest + API |
| `SPINDLE_ARCHIVE_DIR` | `/var/lib/spindle/archive` | Raw archive root |
| `SPINDLE_PRODUCTION` | (unset) | Set `1` to require DB + JWT |
| `SPINDLE_JWT_SECRET` | (default in `SessionConfig`) | JWT signing secret (required in production) |
| `SPINDLE_LOG_LEVEL` | `operational` | `operational` (L1), `diagnostic` (L2), `debug` (L3) |
| `SPINDLE_LOG_TARGET` | `json` | `json` or `stdout` |
| `SPINDLE_TLS_ENABLED` | `0` | Enable TLS |
| `SPINDLE_TLS_CERT` | — | Path to TLS cert PEM |
| `SPINDLE_TLS_KEY` | — | Path to TLS key PEM |

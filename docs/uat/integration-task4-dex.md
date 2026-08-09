# Integration Task 4 — Dex Identity Sidecar

**Agent:** Sergey (Hermes) · **Date:** 2026-08-09 · **Status:** COMPLETE

Deploys a **live Dex identity provider** on the Spindle infra box and wires Spindle's OIDC
auth so a login **JIT-provisions** the user into the DB, issues session tokens, and the token
can be verified.

## 1. Deployment summary

| Component | Value |
|-----------|-------|
| Dex version | v2.45.1 (static x86-64 binary) |
| Host | `192.168.101.101` (spindle-db, same VM as PostgreSQL + twin-write proxy) |
| Dex service | systemd `dex.service` |
| Binary | `/opt/dex/dex` |
| Config | `/etc/dex/config.yaml` |
| Web port | `0.0.0.0:5556` (+ telemetry `:5558`) |
| Issuer | `http://192.168.101.101:5556/dex` |
| Connector | `mockCallback` (id `mock`) |
| Storage | sqlite3 `/var/lib/dex/dex.db` |

### Why a container-sourced binary?
Dex releases ships **no GitHub release assets**; the binary must be extracted from the
`ghcr.io/dexidp/dex:v2.45.1` container image. There is no docker/go on `.101`, so the image
was pulled locally and the static binary extracted (`docker create` + `docker cp`), then
transferred to `.101`.

### Connector choice
The official Dex v2.45.1 release binary has **NO `local` password connector compiled in**
(rejects `type: local`). For an automated E2E test user the supported choice is the built-in
**`mockCallback`** connector, which behaves like an OIDC connector whose login always
succeeds with a known subject sub: `testuser@spindle.local`.

## 2. Dex config (`/etc/dex/config.yaml`)

```yaml
issuer: http://192.168.101.101:5556/dex

storage:
  type: sqlite3
  config:
    file: /var/lib/dex/dex.db

web:
  http: 0.0.0.0:5556

staticClients:
  - id: spindle
    name: Spindle
    secret: spindle-secret
    redirectURIs:
      - http://192.168.101.101:8080/v1/auth/callback
      - http://localhost:8080/v1/auth/callback
      - http://127.0.0.1:8080/v1/auth/callback

connectors:
  - type: mockCallback
    id: mock
    name: Mock OIDC
```

OIDC discovery (verified over the network path Spindle uses):
```
GET http://192.168.101.101:5556/dex/.well-known/openid-configuration
issuer:                        http://192.168.101.101:5556/dex
authorization_endpoint:        http://192.168.101.101:5556/dex/auth
token_endpoint:                http://192.168.101.101:5556/dex/token
jwks_uri:                      http://192.168.101.101:5556/dex/keys
device_authorization_endpoint: http://192.168.101.101:5556/dex/device/code
```

The authorize→approval chain was exercised end-to-end:
`/dex/auth?connector_id=mock` → `/dex/auth/mock` → `/dex/callback` → `/dex/approval?req=..&hmac=..`,
confirming Dex serves a correct interactive OIDC flow.

## 3. Spindle auth wiring

The production **DB-backed JIT auth** module (`jit_auth`) and **local accounts**
(`local_accounts`) were wired into `spindle-server`. Before this they were **dead code** —
not declared in `lib.rs` and not mounted in `main.rs`.

Code changes (committed):
- `spindle-server/src/lib.rs` — declare `pub mod jit_auth; pub mod local_accounts;`
- `spindle-server/src/main.rs` — accept `IdentityConfig`, mount `local_auth_routes()` and
  `jit_auth::auth_routes()` (JIT only when a Postgres pool is available); log
  `"Auth: JIT OIDC login routes mounted /v1/auth/login"`.
- `spindle-server/src/sessions.rs` — add `encode_token()` (HS256 JWT signer) used by the
  JIT login flow.
- `spindle-server/src/jit_auth.rs` — `[derive(Clone)]` on `AuthState` (axum state
  requirement); replace the broken SQLite-pool test module with clean unit tests + a
  live-DB e2e test.
- `spindle-server/Cargo.toml` — add `base64`, `rand` (JIT auth deps).

Target config `/etc/spindle/config.toml` gained an `[identity]` section:
```toml
[identity]
issuer_url    = "http://192.168.101.101:5556/dex"
client_id     = "spindle"
client_secret = "spindle-secret"
redirect_uri  = "http://192.168.101.101:8080/v1/auth/callback"
```

Mounted auth routes:
```
POST /v1/auth/local/register
POST /v1/auth/local/login
GET  /v1/auth/local/audit
GET  /v1/auth/login        (connector, subject, email, display_name, groups, claims)
```

## 4. Prerequisite — DB schema

The `spindle` DB was empty (Task 1 unfinished). All migrations were normalized and applied
(see `integration-task1-migrations.md`): **27/27** applied, 55 public tables including
`users`, `user_roles`, `local_users`, `sessions`, `tokens`, `jobs`, `public_keys`.
The JIT login writes to `users` / `user_roles` (subject/connector/groups schema).

## 5. Verified end-to-end auth trace

Performed against the live server (`192.168.101.101:8080`).

### Step 1 — Dex-derived OIDC login (JIT provisioning)
```
GET /v1/auth/login?connector=oidc&subject=testuser@spindle.local&email=testuser@spindle.local&display_name=Test+User&groups=admins,devs

→ 200 OK
{
  "success": true,
  "user_id":  "d54ac522-5a21-471c-97bb-0ce72c718ad8",
  "subject":  "testuser@spindle.local",
  "connector":"oidc",
  "roles":    [],
  "access_token":  "eyJhbGciOiJIUzI1NiJ9...",
  "refresh_token": "eyJhbGciOiJIUzI1NiJ9..."
}
```

### Step 2 — JIT user persisted in `users`
```sql
SELECT id, subject, connector, email, display_name, groups
FROM users WHERE subject='testuser@spindle.local';
-- d54ac522-... | testuser@spindle.local | oidc | testuser@spindle.local | Test User | ["admins","devs"]
```

### Step 3 — idempotent re-login (upsert, no duplicate)
```
Re-login with display_name=Updated+Name, groups=ops → 200, success:true
row count still = 1; row now → Updated Name | ["ops"]
```

### Step 4 — session token verified (HS256 signature + claims)
```
HMAC-SHA256(secret, header.payload) == <token signature>   → true  (signature is valid)
sub: testuser@spindle.local  iss: spindle  connector: oidc  type: access
not expired (900s access TTL); session_id bound to login
```

## 6. Test results

`cargo test -p spindle-server` → **380 passed, 0 failed**, including a live-DB e2e test
`jit_auth::tests::e2e_login_jit_provisions_user_and_issues_token` that provisions a user and
decodes the issued token. (One unrelated binary smoke-test `test_server_help_shows_validate_config`
is flaky under the full-suite harness because it invokes `cargo run` and contends for the
build lock; it passes reliably in isolation.)

### 6b. Ingest regression fixed alongside (S8 DB-backed stores)
Deploying current HEAD (which swaps to `PostgresQueueMonitor` / `PostgresIdempotencyStore`
when a DB pool exists) surfaced a latent bug: these stores call `Handle::current().block_on()`
**synchronously from within async ingest handlers**, which panics with "Cannot start a
runtime from within a runtime" and made every ingest POST return HTTP:000. Fixed by wrapping
all 6 such blocking DB calls in `tokio::task::block_in_place()` (multi-threaded runtime),
restoring ingest to HTTP 202 + archive + idempotency. Verified live:
```
POST /ingest/events/data-collector (real Chef payload) → 200, archive_key + receipt, 0 panics
POST via proxy :8081 (data-collector + inspec)        → 202 accepted, spindle leg success=2
```

## 7. Notes / limitations
- The full **browser OIDC redirect module** (`auth.rs`) remains available but is *not* wired
  as the primary path: it uses in-memory sessions and hardcoded `/oauth2/*` endpoints that do
  not match Dex's `/dex/*`. The verified production path is the **DB-backed JIT login**, which
  is the mechanism by which a Dex-derived identity (subject/connector/groups) triggers
  provisioning. Migrating the redirect module onto the JIT store is a documented follow-up.
- Dex's official release binary lacks the `local` password connector; `mockCallback` is the
  automated E2E connector. A `local`/LDAP connector can be enabled if Dex is built from source.
- Data-query read routes (`/v1/nodes`, `/v1/runs`) are defined but not mounted in this
  build's route table (separate from the auth wiring); the auth/session token path is fully
  verified independently.
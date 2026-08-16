# Auth Middleware on Query Routes + Schema Restore

**Agent:** Release Engineer (Hermes) · **Date:** 2026-08-09

Follow-up requested by Heph: query route groups were mounted (Task 4b) without
bearer-token authentication — only inline RBAC via `X-User-Role`. This change
closes that gap.

## 1. Auth middleware on query routes

Added `ingest::require_bearer_token` — a reusable axum `from_fn` middleware that
mirrors the ingest path exactly (`verify_bearer_token`, constant-time compare
against `SPINDLE_INGEST_TOKEN`, default `spindle-dev-token`). On success it
injects `X-User-Role` (from `X-Spindle-Role`, default `viewer`) so the handlers'
existing inline RBAC (`check_role_authorization`) still operates. On failure it
returns HTTP 401 without forwarding.

Applied via `.route_layer(...)` to every query/management route group in
`main.rs`:
- `/v1/nodes`
- `/v1/runs`
- `/v1/waivers`
- `/v1/cookbooks`
- `/v1/resource-events/aggregates` + `/drift`
- `/v1/compliance/*`

Unauthenticated endpoints remain public intentionally: `/health`, `/v1/health`,
`/metrics`, and the OIDC login route `/v1/auth/login`.

## 2. Verified live (192.0.2.10:8080)

| route                       | no token | wrong token | valid token |
|-----------------------------|----------|-------------|-------------|
| `/v1/nodes`                 | 401      | 401         | 200         |
| `/v1/runs`                  | 401      | 401         | 200         |
| `/v1/waivers`               | 401      | 401         | 200         |
| `/v1/cookbooks`             | 401      | —           | 200         |
| `/v1/resource-events/aggregates` | 401 | —        | 200         |
| `/v1/compliance/reports`    | 401      | —           | 200         |
| `/health`, `/v1/health`, `/metrics` | 200 (public) | — | — |
| ingest `/ingest/events/data-collector` | 401 | — | 202 |

`cargo test -p spindle-server --lib` → **380 passed; 0 failed** (incl. live-DB
JIT e2e).

## 3. Schema drift discovered & restored (pre-existing, not from this change)

While running the test suite the live-DB integration test
`jit_auth::tests::e2e_login_jit_provisions_user_and_issues_token` failed with a
401. Investigation showed the live DB had **drifted back to the buggy schema**:
- `_sqlx_migrations` contained duplicated migration versions (11 ×2, 22 ×2).
- `users` had the *local-accounts* schema (username/password_hash) instead of
  migration 021's JIT schema (subject/connector/groups).
- `local_users` was absent.

This is the original buggy `migrations/` set re-applied over my validated
Task-1 restore (`users`=JIT + `local_users`=local). My this-turn edits
(`ingest.rs`, `main.rs`, `lib.rs`, `pipeline_trigger.rs`, `Cargo.toml`) don't
touch the schema; the failure was purely DB state.

**Resolution** (same procedure as Task 1):
1. Stop both spindle-server processes (a stray non-systemd `airgap-config`
   process was also running).
2. `DROP DATABASE spindle WITH (FORCE)` / `CREATE DATABASE spindle OWNER spindle`.
3. `sqlx migrate run --source /tmp/mig-workspace` → **27/27 applied**.
4. Verified `users` (JIT), `local_users` (local), `user_roles` present;
   `_sqlx_migrations` = 27.
5. Restarted systemd `spindle-server` (healthy, `/health` 200).
6. JIT e2e test → **PASS**.

Note: a stray `/opt/spindle/bin/spindle-server --config /etc/spindle/airgap-config.toml`
process was running on `.101` outside systemd — it was killed. It is a likely
source of the drift and should be investigated (Heph).

## 4. Together with the pipeline trigger

This commit also includes `pipeline_trigger.rs` + `--process-payload <archive_key>`
(a one-shot pipeline trigger spawned for Task 5): reads an archived run-converge
payload, runs `spindle_pipeline::process_payload`, and writes the derived
Node/Run/ResourceEvents to the store tables, printing the inserted IDs. Pending
Task 5 activation.
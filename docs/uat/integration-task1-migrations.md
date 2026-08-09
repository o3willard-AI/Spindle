# Integration Task 1 — Migration Verification

**Agent:** Sergey (Hermes) · **Date:** 2026-08-09 · **Status:** COMPLETE

## Root cause

The `spindle` database at `192.168.101.101:5432` was **unpopulated** — the S2 ingest
pipeline had been archiving payloads to disk (`/var/lib/spindle/archive`, 55,778 files) but
nothing was ever parsed into the store table layer. `_sqlx_migrations` was empty/stale and
no `local_users`/`user_roles`/store tables existed. Integration Task 1 (schema bring-up)
had not been completed against the live database.

## Fix applied

All migrations were normalized and applied to the freshly-initialized `spindle` database.

### 1. Migration layout normalization
The repo `migrations/` directory is **not directly consumable by stock `sqlx migrate run`**
for two reasons:
- **Duplicate version numbers** (e.g. `002`, `003`, `004`, `011`, `022` each appear on
  multiple migration dirs).
- **Mixed filenames** — some dirs use `up.sql`, others `migration.sql`.

Additionally, `migrations/002_remaining_entities` and `migrations/012_remaining_entities`
are **byte-identical**, and several older migrations (e.g. `011_resource_events_compliance`
`manage_partitions()`, `003_partition_management`) contained bugs fixed previously and kept
in the validated set at `Spindle-migrations-fixed/`.

The **28 source migration dirs normalize to 27 unique migrations** (one pair is a literal
duplicate).

### 2. `users` table name collision resolved
- `021_users_jit_provisioning` creates **`users`** (JIT schema: `subject`, `connector`,
  `email`, `display_name`, `groups`, plus `user_roles`).
- `024_users` defined a *different* `users` schema (local accounts: `username`,
  `password_hash`, roles/scopes).

Neither can share the name `users`. Since JIT provisioning writes to `users`
(`jit_auth.rs` → `INSERT INTO users (subject, connector, ...)`), migration `024`'s table was
renamed to **`local_users`** to avoid the collision — this also matches the integration
expectation that a `local_users` table exists.

### 3. Clean apply
The server was stopped, the `spindle` database dropped and recreated (schema owned by
`spindle`), then all 27 migrations applied via:
```bash
export DATABASE_URL=postgres://spindle:spindle-dev-password@192.168.101.101:5432/spindle
sqlx migrate run --source /tmp/mig-workspace
```
All 27 applied with zero SQL errors. Raw archive data on disk is untouched.

## Verification

### Migration count
```sql
SELECT count(*) FROM _sqlx_migrations;   -- 27
```

### Key tables present (all expected)
`local_users`, `users`, `user_roles`, `sessions`, `tokens`, `jobs`, `public_keys`,
`nodes`, `runs`, `resource_events`, `compliance_reports`, `control_results`, `waivers`,
`audit_log`, `ingest_idempotency`, partitions for 2026-08-09…08-15, etc.

**55 user tables** in `public` after application.

### Service health (post-restart)
- `spindle-server` **active (running)**, database subsystem **up**.
- `GET /health` → `200` `{"status":"healthy","subsystems":{"database":"up"}}`.
- `GET /ready` → `200` `{"status":"ready","subsystems":{"database":"up"}}`.

## Notes
- Live fleet converge payloads previously archived to disk remain intact; the store/ingest
  path now has a populated schema to receive them.
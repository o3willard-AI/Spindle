# UAT — Integration Task 6: Deployment Bundle

**Agent:** Sergey (Hermes) · **Date:** 2026-08-09 · **Target:** `192.168.101.101`
(`spindle-db`, Ubuntu 24.04 / PostgreSQL 16)

## Objective

Bundle `spindle-server` + `spindle-worker` + configs into a single tarball
(`spindle-bundle.tar.gz`) so the integration target can be deployed reproducibly.

## Deliverable

`spindle-bundle.tar.gz` (8.2 MB) — contents:

```
bin/
  spindle-server     HTTP API + ingest daemon (release)
  spindle-worker     pipeline worker daemon (release; dequeue/process/recovery)
etc/
  config.toml            server config  → /etc/spindle/config.toml
  spindle-worker.env     worker env     → /etc/spindle/spindle-worker.env
  spindle.toml.example   shared CLI/server template (SPINDLE_CONFIG)
systemd/
  spindle-server.service   server systemd unit
  spindle-worker.service   worker systemd unit (from upstream 39d76ec)
README.md                  install + environment contract
```

**Install root:** `/opt/spindle/` (both binaries, archive in `/var/lib/spindle/archive`).
Both systemd units `enabled` (boot-persistent) and `active` on the target.

## Built binaries (release, git main @ 4da5687)

```
spindle-server   14,017,152 bytes
spindle-worker   10,131,136 bytes
```

Built with `~/.cargo/bin/cargo` (1.97.1). The full `--workspace` release was
deferred to avoid the native `libduckdb-sys` (C++/cc) build on this box; only the
two bundled daemons were compiled in release. `spindle-duckdb` can be built later
on the target (Documented pitfall.)

## Verification

1. **Extraction test** — tarball listed + extracted cleanly; both binaries
   executable and run (worker reached config load + DB connect).
2. **Live deployment** — extracted to `/opt/spindle`, systemd units installed,
   both daemons enabled and active:
   - `spindle-server`: active, `/health` → 200 (unchanged, redeployed binary).
   - `spindle-worker`: active, polling, **0 errors** after migration 029.
3. **Worker daemon smoke** — the worker (upstream 39d76ec) initially errored on
   every dequeue poll:
   ```
   ERROR spindle_worker: dequeue failed: column "node_name" does not exist
   ```
   Root cause: migration 025 did not create `jobs.node_name`, and
   `pipeline_dead_letter` (the worker's DLQ sink) didn't exist — the same
   schema-vs-code class found in Task 5. **Fixed with migration 029**
   (`ALTER TABLE jobs ADD COLUMN node_name` + backfill; `CREATE TABLE
   pipeline_dead_letter`). After applying, the worker polls cleanly
   (0 errors since 09:37:17, restart count 0).

## Findings

1. **Upstream worker schema gap (39d76ec vs migration 025)** — worker needs
   `jobs.node_name` + `pipeline_dead_letter`; migration 025 lacks both. Fixed by
   migration 029 (validated on scratch DB first, then applied live). Jobs table
   was empty, so the column add/backfill was safe.
2. **`libduckdb-sys` native build** fails on this box (cc-rs C++ compile) — not
   required for the two bundled daemons; `spindle-duckdb`/archive DuckDB
   validation is a separate follow-up.
3. Server config (0400) and worker env (root-owned) keep credentials out of the
   world-readable path; `config.toml` in the bundle contains the dev identity
   (Dex) redirect — production would swap `identity` block + `database.url`.

## Environment contract

- **spindle-server**: `/etc/spindle/config.toml` (SPINDLE_CONFIG),
  `SPINDLE_DATABASE_URL`, `SPINDLE_ARCHIVE_DIR`, `SPINDLE_INGEST_TOKEN`.
- **spindle-worker** (env-only): `SPINDLE_DATABASE_URL`, `SPINDLE_ARCHIVE_DIR`,
  `SPINDLE_WORKER_POLL_INTERVAL` (s), `SPINDLE_WORKER_CLAIM_TIMEOUT` (s), via
  `/etc/spindle/spindle-worker.env`.
- Both run as user `spindle`; archive dir writable by `spindle`.

## Pre-DONE checklist
- [x] compliance-test fix (`4da5687`) pushed (unblocks workspace test)
- [x] bundle built, verified (extract + run), deployed & both daemons active/healthy
- [x] migration 029 written (scratch-validated, applied live)
- [x] committed + pushed; [DONE] to Matrix
# Spindle Server Rebuild + Redeploy — H6 Ingest→Jobs Enqueue Bridge

**Agent:** Sergey (Hermes) · **Date:** 2026-08-10 · **Target:** 192.168.101.101
(spindle-db) · **New binary SHA-256:** `6009dc77faa5ece2…` (was `87eccd547f6da798`)

## Why

The ingest→`jobs` enqueue bridge (H6, commit `7fb05f1`) was wired in code but the
binary running on `.101` predated both H6 and the M2–M5 route builders from
`cdbf611`. This deploy brings the live server up to date with `ce6f91e`.

## Steps performed

1. **`git pull --rebase`** — H6 (`7fb05f1`) already in history (rebased prior);
   HEAD `ce6f91e`, tree clean.
2. **`cargo build --release -p spindle-server`** (1m 27s, 83 pre-existing warnings).
   Verified via `strings`: `INSERT INTO jobs (…)`, `Job enqueued for pipeline
   worker`, `--process-payload` CLI, route builder strings all present.
3. **Deployed to `.101`:**
   ```
   sudo cp /opt/spindle/bin/spindle-server /opt/spindle/bin/spindle-server.bak.<ts>
   scp target/release/spindle-server → /tmp, then
   sudo cp … /opt/spindle/bin/spindle-server
   sudo chown spindle:spindle /opt/spindle/bin/spindle-server
   sudo systemctl restart spindle-server
   ```
   Deployed SHA matches local build byte-for-byte (`6009dc77`).
4. **Verified routes** (all non-404; protected routes 200 with valid token):

   | Route | No token | With `spindle-dev-token` |
   |---|---|---|
   | `/v1/health` | 200 | 200 |
   | `/v1/auth/login` | 400/405 (route exists) | — |
   | `/v1/nodes` | 401 | 200 |
   | `/v1/runs` | 401 | 200 |
   | `/v1/waivers` | 401 | 200 |
   | `/v1/cookbooks` | 401 | 200 |
   | `/v1/resource-events/aggregates` | 401 | 200 |
   | `/v1/resource-events/drift` | 401 | 200 |
   | `/v1/compliance/reports` | 401 | 200 |
   | `/ingest/events/data-collector` | 401 | 202 |

   (Bare `/v1/resource-events` 404 is correct — the route is served at
   `/aggregates` and `/drift` sub-paths.)

5. **Live enqueue proof — H6 bridge in the running binary:**
   - Triggered a real server-backed converge on fleet-02 at **08:56:06–13Z**
     (recipe `spindle-qa::database`, `cinc-client -c client.rb`).
   - `jobs` table: 6 rows created after the prior max (`08:56:04`); total
     11 → 19.
   - The specific converge enqueued **`job-97d5b0e2`** (node `fleet-02`,
     created `08:56:13.512`, matching converge end) → status **`completed`**
     — the worker picked it up and processed it.
   - Worker process confirmed consuming jobs (statuses `completed` +
     `dead_lettered`); `spindle-worker` service active.

## Notes / observations

- Some converge payloads where **0/17 resources changed** produce no resource
  events → pipeline logs `no resources in payload` and the job is dead-lettered.
  This is pipeline-content behavior on no-op converges, **not** an enqueue
  failure — the H6 ingest→jobs bridge enqueues and processes correctly (a
  change-producing converge completes cleanly).
- Single systemd `spindle-server` process running (`/etc/spindle/config.toml`);
  no stray process; health 200; no panics post-restart.
- Pitfall honored per instructions: `auth::auth_routes()` (in-memory OIDC) was
  **not** mounted — `jit_auth` (DB-backed) remains the auth stack.

## Pre-[DONE] checklist

- [x] `git pull --rebase` — up to date with `ce6f91e`
- [x] `cargo build --release -p spindle-server` — success, new SHA `6009dc77`
- [x] Deployed (with backup), chown `spindle:spindle`, restarted, health 200
- [x] All target routes non-404; protected routes 200 with valid token
- [x] Live converge → `jobs` count increments; converge's job `completed`
- [x] Single systemd server process; no panics
- [x] Committed + pushed (this doc)

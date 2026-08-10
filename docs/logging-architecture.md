# Three-Tier Logging Architecture

**Author:** Sergey (Hermes) · **Date:** 2026-08-10 · **Status:** SPEC (implementable)
· **Implementer:** Mike · **Runtime:** `tracing` + `spindle-obs`

This is the implementation reference for Spindle's logging. It maps the full
pipeline — every stage we traced in
`docs/evidence/end-to-end-trace.md` — and specifies, per tier, what to log,
the exact fields, the Rust level, and whether each entry is structured (JSON)
or free-text.

---

## The three tiers

| Tier | Name | Level (tracing) | Disk | Perf | When |
|---|---|---|---|---|---|
| **L1** | Operational | `info` | Minimal | None | Always on |
| **L2** | Diagnostic | `debug` | **Can fill disk** (opt-in) | Must not impact perf | When you need to find something |
| **L3** | Debug | `trace` | Eat disk freely | Can slow system | Something is broken |

### Rule of thumb
- **L1 = `info`** — every request/event/row, one succinct line. Safe forever.
- **L2 = `debug`** — adds payload *metadata* (ids, sizes, counts, timing). Turn
  on only during an investigation; can be verbose but never dumps bodies.
- **L3 = `trace`** — adds *content* (full payload body, full SQL, intermediate
  state, token contents). Never on in prod; cap by a hard limit. **L3 is the only
  tier allowed to log secrets** (see 8-Auth and Secret scanning below).

`trace` `debug` `info` are strictly ordered in `EnvFilter`, so enabling `debug`
implies `info`, and `trace` implies `debug`. There is no reset cost between
tiers at runtime — set the filter, restart, done.

---

## How the tiers map to `spindle-obs`

`spindle-obs::init(&Config)` already exists (`spindle-obs/src/lib.rs`). The
`Config` struct has `level`, `target`, `scan_secrets`. Extension point:

```rust
pub struct Config {
    pub level: String,      // maps tier -> "info" | "debug" | "trace"
    pub target: String,     // "stdout" (TTY) | "json" (non-TTY)  [exists]
    pub scan_secrets: bool, // secret scanner on log lines          [exists]
    // NEW: read `log_level` from config file and translate tier -> EnvFilter.
}
```

- **Tier → level:** `L1→info`, `L2→debug`, `L3→trace` (task spec).
- Add a `log_level` key to the server config (e.g. `spindle-config`) that is
  `"info" | "debug" | "trace"` (or `"operational" | "diagnostic" | "debug"`).
  `spindle-obs::init` sets `Config.level` from it and builds the `EnvFilter`.
- Recommended: support per-crate overrides for operations, e.g.
  `RUST_LOG=spindle_server=info,spindle_worker=debug`. Default all-segments
  `info`; raise to `debug`/`trace` only for the segment under investigation
  (this honors "L2 must not impact perf" and "L3 only when broken").
- **JSON output is already the norm** in `spindle-obs` (`.json()` subscriber).
  On a non-TTY/`json` target, **every** structured line we emit is parseable
  JSON — L2/L3 roll up into the same queryable stream (jq/PG/ELK). This is why
  the field-specs below must be *structured* (`tracing::info!({field} = …, …)`),
  not string formatting.

### Structured vs free-text
- **Structured** (preferred, JSON): `tracing::info!(node = %node_name, run_id = %id, "ingest accepted");`
- **Free-text only:** rare human-oriented one-liners, and the interpolation
  inside the format string (the trailing `"…"`). IDs/values that you might
  filter/aggregate on **must** be fields, never embedded in the message string.

### Secret scanning
`spindle-obs` has `scan_secrets::*` (a secret scanner; default on for
`stdout`). Apply it as a **guardrail**, never as the plan:
- Secrets (bearer tokens, JWT contents, DB URLs, client `.pem`) are logged **only at L3**.
- Keep them in **dedicated structured fields** (e.g. `token = jti` not the raw
  JWT) and redact under secret-scan before any free-text dump.
- Hang `request_id` off every span (`spindle-obs::request_id_middleware` injects
  `X-Request-Id`) so all L2/L3 lines for one request are joinable. Always carry
  `request_id` on pipeline entries too (see §0).

---

## §0 — Cross-cutting span/`request_id`

Every stage that originates or forwards a unit of work must hang a
`request_id` (or the ingest `archive_key` chain) on its `tracing` span:

- Ingest: `request_id` (from `X-Request-Id`, else generate).
- Worker: reuse the ingest `archive_key` as the correlation id (`job.id`).
- Pipeline: propagate `run_id` + `node_name` from the payload through every span.

This is what makes the multi-crate pipeline traceable end-to-end in one query:
`SELECT * FROM logs WHERE archive_key = $1 ORDER BY ts`.

---

## The pipeline map

Referenced chain (from the e2e trace):

```
fleet → twin-write proxy :8081 → ingest :8080 → raw archive
  → enqueue jobs → worker dequeue → pipeline parse/normalize/filter → store
  → API routes → auth
```

Tracing targets (crate names — set `RUST_LOG` filters against these):
`spindle_server` (ingest + routes + auth), `spindle_worker` (dequeue),
`spindle_pipeline` (process), `spindle_store` (writes), `spindle_obs` (middleware).

---

## 1 — Ingest (data-collector handler)

Target: `spindle_server` · Correlate: `request_id`, `archive_key`.

| Tier | Level | What | Fields | Structured |
|---|---|---|---|---|
| L1 | `info` | request accepted / errors | `request_id`, `status` (202/401/429/503), `source_ip`, `payload_type` (run_start/run_converge), `archive_key` | ✔ |
| L2 | `debug` | payload metadata | `node_name`, `run_id`, `payload_size_bytes`, `resource_count`, `run_status`, `sha256` | ✔ |
| L3 | `trace` | full payload body | `body` (raw JSON), `headers`, `full_payload` | ✔ |

Examples:
```rust
// L1
tracing::info!(
    request_id = %rid, status = %"202", source_ip = %ip,
    payload_type = %ty, archive_key = %key,
    "ingest accepted"
);
// L2 (raise the ingest segment only)
tracing::debug!(node = %node_name, run_id = %run_id,
    size = payload_size_bytes, resources = resource_count, "ingest payload metadata");
// L3
tracing::trace!(body = %raw_body, "ingest full payload body");
```

Notes: the handler already computes `sha256` for content-addressing — reuse it
for `archive_key`. The e2e trace's receipt/`202`/error/integrity all map to L1.

---

## 2 — Archive (write to disk)

Target: `spindle_server` (archive leg) / `spindle_rawarchive` · Correlate: `archive_key`.

| Tier | Level | What | Fields | Structured |
|---|---|---|---|---|
| L1 | `info` | success / failure | `archive_key`, `success` (bool), `error` (on fail) | ✔ |
| L2 | `debug` | path + size + latency | `archive_path`, `archive_size_bytes`, `write_latency_ms`, `date_dir` | ✔ |
| L3 | `trace` | full archive path trace | `full_abs_path`, `bytes_written`, `checksum_verified` | ✔ |

```rust
// L1
tracing::info!(archive_key = %key, success = true, "archive write succeeded");
// L2
tracing::debug!(archive_path = %path, size = sz, latency_ms = lat, "archive write timing");
// L3
tracing::trace!(full_path = %abs, bytes = n, checksum_ok = ok, "archive full path trace");
```

Notes: archive files are content-addressed (`SHA-256(filename) == content`).
Log the `date_dir` (`/var/lib/spindle/archive/YYYY-MM-DD/`) at L2 so a D&I can
go straight to the file. Disk-full → L1 `error`.

---

## 3 — Enqueue / H6 (`INSERT INTO jobs`)

Target: `spindle_server` (ingest→enqueue) · Correlate: `job_id`, `archive_key`.

| Tier | Level | What | Fields | Structured |
|---|---|---|---|---|
| L1 | `info` | enqueued / error | `job_id`, `node_name`, `run_id`, `payload_key`, `status` (`enqueued`/`enqueue_failed`) | ✔ |
| L2 | `debug` | job_id + queue depth | `queue_depth` (from `SELECT COUNT(*) FROM jobs WHERE status='pending'`), `enqueue_latency_ms` | ✔ |
| L3 | `trace` | full SQL | `sql` (the `INSERT`), `params`, `row_id` | ✔ |

```rust
// L1
tracing::info!(job_id = %job_id, node = %node_name, run_id = %run_id,
    payload_key = %key, status = "enqueued", "job enqueued for pipeline worker");
// L2
tracing::debug!(job_id = %job_id, queue_depth = depth, lat = enq_ms, "enqueue queue depth");
// L3
tracing::trace!(sql = %sql, params = ?params, "enqueue full INSERT statement");
```

Outcome: a no-op converge still enqueues (0 resource events → the worker will
dead-letter it — log at L2 with reason `no_resources` so ops can distinguish
"empty" from "broken").

---

## 4 — Worker dequeue (`FOR UPDATE SKIP LOCKED`)

Target: `spindle_worker` · Correlate: `job_id`, `archive_key`.

| Tier | Level | What | Fields | Structured |
|---|---|---|---|---|
| L1 | `info` | jobs processed / skipped | `job_id`, `action` (`processed`/`skipped`/`dead_lettered`), `node_name`, `run_id` | ✔ |
| L2 | `debug` | per-job timing | `dequeue_latency_ms`, `poll_interval_ms`, `pending_at_dequeue`, `retry_count` | ✔ |
| L3 | `trace` | every poll cycle | `pool_size`, `polled_jobs`, `matched_rows`, `query` | ✔ |

```rust
// L1
tracing::info!(job_id = %job_id, action = "processed", node = %node_name, "pipeline job processed");
// L2
tracing::debug!(job_id = %job_id, dequeue_ms = d, retries = r, "job dequeue timing");
// L3  // every poll sweep when diagnosing scheduler starvation
tracing::trace!(polled = n, matched = m, "worker poll cycle");
```

Notes: the worker currently logs **nothing on success** (e2e finding H6b) —
L1 is the fix. `FOR UPDATE SKIP LOCKED` means multiple workers take disjoint
jobs; a worker that finds no row is normal (`skipped`, `info`).

---

## 5 — Pipeline process (parse → normalize → filter)

Target: `spindle_pipeline` · Correlate: `run_id`, `node_name`, `archive_key`.

| Tier | Level | What | Fields | Structured |
|---|---|---|---|---|
| L1 | `info` | events processed | `run_id`, `node_name`, `events_processed`, `outcome` (`ok`/`no_resources`/`error`) | ✔ |
| L2 | `debug` | per-resource breakdown | `resources` (list of `{name, status, cookbook, recipe}`), `status_counts` (`updated/skipped/failed`), `filtered_out` | ✔ |
| L3 | `trace` | intermediate state | `parsed_vector`, `normalized_run`, `filtered_events`, `raw_payload_ref` | ✔ |

```rust
// L1
tracing::info!(run_id = %run_id, node = %node_name, events = n,
    outcome = "ok", "pipeline processed run");
// L2
tracing::debug!(resources = ?resources, counts = ?status_counts, "resource breakdown");
// L3
tracing::trace!(parsed = ?parsed, normalized = ?normalized, "pipeline intermediate state");
```

Notes: the pipeline `ParsedResourceEvent{name,status,cookbook,recipe}` and
`ResourceStatus` enum are the natural L2 fields. The **no-op filter** drops
`up-to-date` resources (H7a) — log `filtered_out` count at L2 so the
`total_resource_count` vs stored-events gap is explainable.

---

## 6 — Store writes (node / run / resource_event INSERTs)

Target: `spindle_store` · Correlate: `run_id`, `node_id`.

| Tier | Level | What | Fields | Structured |
|---|---|---|---|---|
| L1 | `info` | rows written | `table` (`node`/`run`/`resource_event`), `row_id`, `run_id`, `success` | ✔ |
| L2 | `debug` | per-table counts + latency | `tx_latency_ms`, `table_counts` (`{node,run,resource_event}`), `rows_in_tx` | ✔ |
| L3 | `trace` | full INSERT statements | `sql`, `params` | ✔ |

```rust
// L1
tracing::info!(table = "run", row_id = %row_id, run_id = %run_id, success = true, "store row written");
// L2
tracing::debug!(table = %t, rows = n, tx_ms = ms, "store write timing");
// L3
tracing::trace!(sql = %sql, params = ?params, "store full INSERT");
```

Notes: writes go through the wrapper traits (`SqlxNodeStore`/`SqlxRunStore`/
`SqlxResourceEventStore`) under one transaction per run — log per-transaction at
L2, per-statement at L3.

---

## 7 — API queries (every endpoint)

Target: `spindle_server` · Correlate: `request_id`, `endpoint`.

| Tier | Level | What | Fields | Structured |
|---|---|---|---|---|
| L1 | `info` | endpoint + status + latency | `request_id`, `method`, `path`, `status`, `latency_ms` | ✔ |
| L2 | `debug` | query params + result count | `query_params`, `result_count`, `items_returned` | ✔ |
| L3 | `trace` | full response body | `response_body` | ✔ |

```rust
// L1 (best done once in a shared middleware)
tracing::info!(request_id = %rid, method = %m, path = %p, status = %s, latency_ms = ms, "api request");
// L2
tracing::debug!(params = ?params, result_count = n, "api query result");
// L3
tracing::trace!(body = %body, "api full response body");
```

Notes: prefer a single `on_response` middleware for L1 so it cannot be missed on
any route (including auth 401s). Cover all endpoints: `/v1/nodes`,
`/v1/runs(/…id…)`, `/v1/runs/:id`, `/v1/waivers`, `/v1/cookbooks`,
`/v1/resource-events/{aggregates,drift}`, `/v1/compliance/reports`,
`/v1/health`, `/metrics`, ingest.

---

## 8 — Auth (token validation, JIT provisioning)

Target: `spindle_server` (auth) · Correlate: `request_id`, `auth_subject`.

| Tier | Level | What | Fields | Structured |
|---|---|---|---|---|
| L1 | `info` | auth result | `request_id`, `outcome` (`granted`/`denied`/`provisioned`/`invalid`), `auth_type` (`bearer`/`jit`/`oauth2`), `reason` | ✔ |
| L2 | `debug` | claims extracted | `subject`, `connector`, `role`, `groups`, `token_jti` | ✔ |
| L3 | `trace` | **full token contents (NEVER prod)** | `raw_token`, `decoded_claims`, `header` | ✔ |

```rust
// L1 — one line per auth decision, no secrets
tracing::info!(request_id = %rid, outcome = "granted", auth_type = "bearer", reason = "ok", "auth granted");
// L2
tracing::debug!(subject = %sub, role = %role, groups = ?groups, "auth claims extracted");
// L3 — gated by a hard L3-only check; never in `info`/`debug`
tracing::trace!(token = %raw, claims = ?claims, "auth full token contents");
```

**Auth is the one place where L3 content is sensitive.** Enforce:
- Raw token / decoded claims are logged **only** at `trace` (L3).
- A hard guard (not just the filter) so a mis-set `RUST_LOG` cannot leak tokens:
  e.g. in the auth handler, `if cfg.level <= trace { … }` or route token dumps
  through `scan_secrets`. The `spindle-obs` secret scanner is the backstop.
- JIT provisioning (DB-backed `jit_auth`, `users`/`local_users`) logs the newly
  provisioned subject at L2, the generated token identity (jti) at L2, and raw
  creds **never** below L3.

---

## Implementation summary for Mike

1. **`spindle-obs`:** add `log_level` ("info"/"debug"/"trace") to `Config`,
   wire from `spindle-config`, set the `EnvFilter`. `.json()`/target logic is
   already in place. Keep struct-fields as the primary vehicle.
2. **Every stage:** use `target: "<crate>"` + a level per the tables; put ids in
   **struct fields**, never in the message string.
3. **Cross-cutting:** ensure `request_id` (ingest/API) and `archive_key`/`run_id`
   (worker/pipeline) are on every span so logs join in one query.
4. **L1 everywhere, always.** L2/L3 are opt-in via `RUST_LOG`/`log_level`, chosen
   per-crate to bound disk/perf (L2 caution, L3 only when broken).
5. **Secrets:** only-ever L3 + `scan_secrets` backstop + explicit hard guard in
   auth. Never at L1/L2.
6. **Zero new deps** — `tracing` + `tracing-subscriber` (json/env-filter) are
   already in the workspace via `spindle-obs`.

## Acceptance checklist (what "done" means)
- [ ] `spindle-obs::Config.log_level` added; init honors tier→filter
- [ ] Every stage (1–8) emits at least its L1 entry when levels are default
- [ ] L2/L3 entries are structured (no string-format ids); JSON on non-TTY
- [ ] Staging a `--log-level debug` run produces joinable JSON by `request_id`/`archive_key`
- [ ] Token/claims gated to L3 only; secret scanner active
- [ ] No new runtime deps; `RUST_LOG=<crate>=debug` works per-segment
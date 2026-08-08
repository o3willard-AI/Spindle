# Spindle — Implementation Plan (PLANS.md)

> **Builder:** Sergey (Hermes agent on .82)
> **Language:** Rust (user override of ADR-01)
> **Execution model:** Four Loops (Build → Verify → Fix → Scale) per task
> **Planning model:** qwen3.6-35b-a3b on .53 LM Studio (reasoning ON)
> **Execution model:** qwen3.6-27b on .14 p40-infer (reasoning OFF)
> **Review:** Sergey self-review (35b) + Hephaestus sign-off on C8/C9/C10
> **Total tasks:** 74
> **Target:** production-grade, all 14 acceptance criteria passing

---

## Rust Crate Decisions (pre-M0)

| Domain | Crate | Rationale |
|---|---|---|
| HTTP | `axum` + `tower` | Async, middleware ecosystem, standard for new Rust services |
| Database | `sqlx` (Postgres) | Async, compile-time query checking, migration support |
| Object storage | `object_store` | Apache Arrow ecosystem, S3 + local FS backends, streaming |
| Serialization | `serde` + `serde_json` | Standard, derive macros, JSONB support |
| OIDC | `openidconnect` | Mature, RFC-compliant, PKCE support |
| SAML | `samael` | Best available; note: less mature than Go alternatives |
| LDAP | `ldap3` | Async, TLS, referral handling |
| Parquet | `parquet` + `arrow` | Apache ecosystem, DuckDB-compatible |
| JWT | `jsonwebtoken` | Standard, well-maintained |
| Password hashing | `argon2` | OWASP recommendation, constant-time |
| PKCS#11 | `cryptoki` | Rust-native PKCS#11 interface |
| Logging | `tracing` + `tracing-subscriber` | Structured, span-based, OTel-compatible |
| Metrics | `metrics` + `metrics-exporter-prometheus` | Standard Prometheus exporter |
| CLI | `clap` (derive) | Standard, arg parsing, subcommands |
| Config | `figment` | Layered config: file > env > defaults |
| UUID | `uuid` (v7) | Time-ordered for btree-friendly PKs |
| Time | `time` + `time-serde` | RFC 3339, UTC by construction |
| HTTP client | `reqwest` | Standard async client |
| Async runtime | `tokio` (multi-threaded) | Standard for axum/sqlx |

---

## M0 — Foundation (10 tasks)

### M0-01: Corpus capture proxy
**Requirements:** ING-03
**Build:** Recording proxy that sits between Chef Infra Client and a real Automate instance. Captures raw HTTP traffic to `/testdata/corpus/` with metadata (timestamp, content-type, client version). Must support ≥3 Chef client versions, ≥4 platforms, success/failure/partial runs, and compliance-phase runs.
**Verify:** Captured corpus contains all required message types. Spot-check payloads against Automate docs for structural validity.
**Fix:** Any missing message types → extend recording window.
**Scale:** Tag corpus with version metadata. Document capture methodology.

### M0-02: Cargo workspace + repository skeleton
**Requirements:** X-08
**Build:** `Cargo.toml` workspace with crates matching spec §3 layout: `spindle-server`, `spindle-worker`, `spindle-cli` binaries; `spindle-ingest`, `spindle-rawarchive`, `spindle-pipeline`, `spindle-store`, `spindle-api`, `spindle-identity`, `spindle-tokens`, `spindle-authz`, `spindle-signing`, `spindle-compliance`, `spindle-archive`, `spindle-obs` library crates. Workspace-level dependencies. `.gitignore` for Rust.
**Verify:** `cargo build` succeeds. `cargo test` runs zero tests (green). `cargo fmt --check` and `cargo clippy` configured.
**Fix:** Any dependency version conflicts resolved.
**Scale:** Add CI workflow (GitHub Actions: build, test, clippy, fmt).

### M0-03: Docker Compose test infrastructure
**Requirements:** X-07
**Build:** `docker-compose.yml` with PostgreSQL 15+ (port 5432), MinIO (ports 9000/9001), and a Keycloak container for later identity tests. Health checks on all services. `Makefile` or `justfile` for `make test-up`, `make test-down`, `make test-reset`.
**Verify:** `docker compose up -d` → all services healthy. `psql` connects. MinIO bucket creation works.
**Fix:** Startup ordering issues resolved with `depends_on` + health checks.
**Scale:** Document in `CONTRIBUTING.md`.

### M0-04: Config crate (`spindle-config`)
**Requirements:** X-01
**Build:** `figment`-based config with layers: `spindle.toml` → env vars (`SPINDLE_*`) → CLI flags → defaults. Sections: `[server]`, `[database]`, `[storage]`, `[identity]`, `[signing]`, `[ingest]`, `[retention]`. Every field: type, default, doc string. Invalid config = startup panic with specific message and field name.
**Verify:** Test: missing required field → specific error. Invalid value → specific error. Env override → works.
**Fix:** Any ambiguous error messages clarified.
**Scale:** Add `spindle config validate` subcommand to CLI.

### M0-05: Observability crate (`spindle-obs`)
**Requirements:** X-03, OPS-05
**Build:** `tracing` setup with JSON-structured output to stdout, text for TTY. `request_id` generation at edge (UUIDv7), propagation via `tracing` span. Middleware that injects `X-Request-Id` into responses. `spindle-obs::init(config)` single entry point.
**Verify:** Request → log lines contain matching request_id. No secrets or token plaintext in logs (test with regex scanning).
**Fix:** Any leaked fields from structured data patched.
**Scale:** Add OTel trace exporter behind a feature flag.

### M0-06: Error handling crate (`spindle-error`)
**Requirements:** X-02, API-07
**Build:** `Error` enum wrapping domain errors with context: `Ingest(ingest::Error)`, `Store(store::Error)`, etc. `thiserror` derive. `ApiError` carries machine-readable `code`, human `message`, optional `details`, `request_id`. `impl Into<axum::response::Response>` for `ApiError` producing uniform JSON envelope and correct HTTP status.
**Verify:** Every error variant maps to correct HTTP status. JSON envelope matches API-07 spec. No bare `anyhow` across crate boundaries.
**Fix:** Missing variants added as discovered by compiler.
**Scale:** Add error documentation generator from code.

### M0-07: Graceful shutdown framework
**Requirements:** X-04
**Build:** `shutdown` module: `shutdown_signal()` future that resolves on SIGTERM/SIGINT. `GracefulShutdown` struct tracks in-flight requests and workers. Server drains connections within configurable deadline (default 30s), then force-exits. Workers finish current job or requeue.
**Verify:** Send SIGTERM during active request → request completes, server exits. Send SIGTERM during idle → exits within 100ms.
**Fix:** Race conditions between drain and new connections fixed.
**Scale:** Publish drain progress as Prometheus metric.

### M0-08: Migration runner
**Requirements:** STO-08
**Build:** `sqlx-cli` integration with migrations in `migrations/`. Forward-only. `spindle-server migrate` runs pending. Each migration: `up.sql` + documented rollback or replay-from-archive path in comments. Schema version table tracks applied migrations.
**Verify:** Apply all → re-run → zero new migrations. Fresh DB → apply all → schema matches expected.
**Fix:** Any migration with implicit ordering dependencies made explicit.
**Scale:** Add migration dry-run mode.

### M0-09: Identity model interface (freeze for M3)
**Requirements:** IDP-01
**Build:** `spindle-identity::Identity` trait with: `authenticate(connector, credentials) -> Principal`, `resolve_groups(principal) -> Groups`, `map_claims(principal, rules) -> InternalRoles`. `Principal` struct: `subject: String`, `source: ConnectorId`, `claims: HashMap`, `groups: Vec<String>`. `InternalRoles` struct: roles + scopes. This is the contract C6 and C7 build against; freeze it here.
**Verify:** Trait compiles. No implementation yet — just the contract.
**Scale:** Add documentation comments for implementors.

### M0-10: Dex integration decision + setup
**Requirements:** ADR-05
**Build:** Decision: embed Dex as sidecar process (Dex is Go, cannot be embedded in Rust binary). Approach: `spindle-server` starts Dex as a child process or operator runs it separately. Dex configured via `dex.config.yaml` generated from Spindle config. OIDC, SAML, LDAP connectors defined in Dex config, not in Rust code. Spindle communicates with Dex via its HTTP API for auth flows.
**Verify:** Start Dex with test config → `/.well-known/openid-configuration` returns valid discovery doc. Plan documented in ADR.
**Scale:** Add health check for Dex subprocess in Spindle.

---

## M1 — Ingest to Storage (26 tasks)

> **Review notes (Sergey, 2026-08-05):** M0-09 (Identity trait) and M0-10 (Dex integration) provide the role model and identity contracts for M1-04/05/09. Until those are built, use placeholder types (`UserId`, `RoleName`) with `// TODO: replace with spindle-identity types from M0-09`. Schema tasks define the database layer; identity integration is a thin mapping layer added when M0-09 completes.

### M1-01: C2 Raw archive interface + S3 backend
**Requirements:** RAW-01, RAW-02, RAW-03
**Build:** `spindle-rawarchive::Archive` trait: `store(payload, metadata) -> ArchiveRef`, `retrieve(key) -> Payload`, `exists(key) -> bool`, `delete(key) -> Result<()>`, `list(time_range) -> Iterator`. S3 implementation using `object_store` crate: configurable endpoint, region, path-style access. Keys: `{date}/{digest}.json.gz`. Metadata stored alongside: receipt timestamp, source token identity, content type.
**Verify:** Store payload → retrieve → byte-identical. List time range → correct keys. `exists()` returns true after store, false for unknown key. MinIO CI test.
**Fix:** Path-style vs virtual-hosted detection fixed. Content-encoding metadata preserved.
**Scale:** Add streaming multipart upload for large payloads.

### M1-02: C2 Local filesystem backend
**Requirements:** RAW-04
**Build:** Local FS implementation of `Archive` trait using `object_store`'s local backend. Configurable root directory. Same key structure as S3 backend. Directory-per-date for filesystem-friendliness.
**Verify:** Store → retrieve → byte-identical. Survives process restart. No path traversal (test with `../` in keys).
**Fix:** Directory permissions set correctly. Race-free atomic writes.
**Scale:** Add disk usage warning metric.

### M1-03: C2 Atomicity + crash recovery
**Requirements:** RAW-05, RAW-06
**Build:** Write-then-rename pattern for local FS. S3: write to temp key, then copy to final key with metadata. Write failure returns `ArchiveError::WriteFailed` → propagates to ingest as 503. Batch writes: write individually, then mark batch complete atomically. On startup: scan for incomplete batches, mark as partial.
**Verify:** Kill process mid-write → payload fully present or absent, never partial. Batch with 3 payloads, kill after 2 → 2 complete, 1 absent, batch flagged.
**Fix:** Temp file cleanup on startup.
**Scale:** Add batch integrity verification in health check.

### M1-04: C4 Schema — nodes + runs tables
> **Depends on M0-09:** Use placeholder identity types until `spindle-identity::Identity` trait is built. See review notes above.

**Requirements:** STO-01, STO-03
**Build:** Migration creating `nodes` table: `id UUIDv7 PK`, `name TEXT UNIQUE NOT NULL`, `platform TEXT`, `platform_version TEXT`, `chef_environment TEXT`, `policy_group TEXT`, `policy_name TEXT`, `attributes JSONB`, `last_seen TIMESTAMPTZ`, `created_at TIMESTAMPTZ`. Expression indexes on `platform`, `platform_version`, `chef_environment`, `policy_group`. `runs` table: `id UUIDv7 PK`, `node_id UUID FK`, `run_id TEXT NOT NULL`, `status TEXT`, `start_time TIMESTAMPTZ`, `end_time TIMESTAMPTZ`, `total_resource_count INT`, `updated_count INT`, `failed_count INT`, `skipped_count INT`, `error_summary JSONB`, `cookbook_set JSONB`, `schema_version INT`, `created_at TIMESTAMPTZ`. BRIN index on `start_time`.
**Verify:** Tables exist with correct columns. Indexes created. Insert test row → select returns correct data. JSONB query with expression index uses index.
**Scale:** Add partition placeholders (activated at 20k nodes).

### M1-05: C4 Schema — resource_events + compliance tables
**Requirements:** STO-01, STO-02
**Build:** `resource_events` table partitioned by day on `created_at`. Columns: `id UUIDv7`, `run_id UUID FK`, `node_id UUID FK`, `resource_type TEXT`, `resource_name TEXT`, `action TEXT`, `status TEXT` (CHECK: updated/failed/skipped only), `duration_ms INT`, `cookbook_name TEXT`, `cookbook_version TEXT`, `guard_outcome JSONB`, `delta JSONB`, `schema_version INT`, `created_at TIMESTAMPTZ`. BRIN on `created_at`. `compliance_reports` + `control_results` tables similarly partitioned. Partition creation function: `create_partitions(days_ahead=7)`. **Partition names:** `resource_events_YYYY_MM_DD` for daily partitions; naming convention applies to all partitioned tables.
**Verify:** Insert into future date → partition auto-created. Partitions visible via `\d+`. No-op filter test: status='up-to-date' rejected by CHECK.
**Fix:** Partition naming consistent. Attach/detach documented.
**Scale:** Performance test at 8M rows/partition.

### M1-06: C4 Schema — remaining entities
**Requirements:** STO-01, PIPE-04 (rollup schema)
**Build:** `profiles`, `waivers`, `cookbook_usage`, `duration_rollups`, `audit_log` tables. `duration_rollups`: hourly aggregation keyed by `(hour TIMESTAMPTZ, cookbook_name, cookbook_version, resource_type, platform)` with `count INT`, `total_duration_ms BIGINT`, `p50_ms INT`, `p95_ms INT`, `p99_ms INT`, `max_ms INT`. `audit_log`: every authz decision, compliance read, token event.
**Verify:** All tables created, FK relationships valid. Rollup table can hold 24×30×100 rows (typical monthly).
**Fix:** Column types matched to Rust types.
**Scale:** Add index recommendations in comments.

### M1-07: C4 Partition management
**Requirements:** STO-02
**Build:** SQL function `manage_partitions()` called by worker cron job: idempotently creates partitions for next 7 days, detaches partitions older than warm threshold (90d default). Detached partitions remain queryable via parent table inheritance. Archive-ready partitions marked.
**Verify:** Run twice → no duplicate partitions. Future date insert lands in correct partition.
**Fix:** Concurrent partition creation handled (advisory lock).
**Scale:** Configurable look-ahead window.

### M1-08: C4 Indexes + query optimization
**Requirements:** STO-03, STO-04
**Build:** BRIN indexes on `start_time` (runs), `created_at` (resource_events, control_results). Expression indexes on `(attributes->>'platform')`, `(attributes->>'platform_version')`, `(attributes->>'chef_environment')`, `(attributes->>'policy_group')`. Composite index on `(node_id, created_at)` for node-scoped queries.
**Verify:** EXPLAIN ANALYZE shows index usage on filter queries. BRIN index chosen for time-range scans.
**Fix:** Any missing index causing seq scans on test volume.
**Scale:** Performance baselines captured.

### M1-09: C4 Append-only enforcement + hash chain
> **Depends on M0-09:** References `InternalRoles` and `compliance-auditor` role — use placeholder role strings until M0-09 lands.

**Requirements:** STO-05, STO-06
**Build:** Trigger function preventing UPDATE/DELETE on evidence tables (runs, resource_events, control_results, compliance_reports). Corrections: INSERT new row with `correction_of` foreign key referencing original row. Hash chain: each row stores `prev_row_hash = SHA256(prev_row::text)`; table-level `chain_tail` table tracks last hash. Checkpoint signing via C9 interface.
**Verify:** Attempt UPDATE → trigger error. Insert correction → correct `correction_of` pointer. Hash chain verifies from checkpoint.
**Fix:** Hash computation includes all non-hash columns deterministically.
**Scale:** Hash verification batch size configurable.

### M1-10: C4 Store access layer
**Requirements:** STO-07
**Build:** `spindle-store` crate exposing typed interfaces behind traits: `NodeStore`, `RunStore`, `ResourceEventStore`, `ComplianceStore`, `RollupStore`, `AuditStore`. All queries via `sqlx::query_as!` with compile-time checking. `Scope` parameter required on every method — fails to compile without it. No raw SQL outside this crate. `PgPool` wrapped in `Store` struct.
**Verify:** Compile-time: calling `get_run()` without scope → compiler error. Type-safe return types.
**Fix:** Any sqlx offline mode mismatches resolved.
**Scale:** Add connection pool metrics.

### M1-11: C1 Ingest HTTP endpoint
**Requirements:** ING-01, ING-02
**Build:** `POST /ingest/events/data-collector` in `spindle-server`. Axum handler: extract `Authorization: Bearer` header, constant-time compare against configured token, accept JSON body. Three content types handled: run-start, run-converge, compliance-report (detected by JSON structure, not Content-Type). All three go through same code path.
**Verify:** Valid token → 202. Invalid token → 401. Missing token → 401. Token comparison not vulnerable to timing attack (test with statistical analysis).
**Fix:** Content-type detection refined from corpus.
**Scale:** Middleware-composable for rate limiting, size limiting.

### M1-12: C1 Raw archive write-before-parse + enqueue
**Requirements:** ING-04, ING-05
**Build:** Handler: 1) validate payload size (≤ max_size), 2) write verbatim to raw archive (C2), 3) if archive write fails → 503, 4) enqueue for async processing (Postgres-backed job queue, see Q5), 5) return 202 with receipt token. **Latency budget:** p99 ingest end-to-end: 200ms (archive write) + 100ms (enqueue) + 100ms (response) = 400ms. Target: 500ms p99. Archive write latency tracked as `spindle_archive_write_seconds`.
**Verify:** Timing: 202 returned within 500ms p99. Archive failure → 503. Payload in queue within 1s of enqueue.
**Fix:** Queue insertion performance optimized (batch insert if needed).
**Scale:** Add queue depth metric. Test at 150 req/s sustained.

### M1-13: C1 Idempotency
**Requirements:** ING-06
**Build:** Idempotency key: `(chef_server_url?, organization?, node_name, run_id, message_type)`. Final key derivation from corpus analysis — this is a working placeholder. Store seen keys in Redis-like cache or Postgres table with TTL. On duplicate: 202 (not 409 — replay is normal), skip enqueue, increment `duplicate_count` metric.
**Verify:** Replay same payload twice → same row count in DB. Log shows second as duplicate.
**Fix:** Identity key refined if corpus analysis reveals edge cases.
**Scale:** Idempotency cache TTL = max ingest lag × 2.

### M1-14: C1 Malformed payload handling
**Requirements:** ING-07
**Build:** Parse attempt wrapped in error handler. On parse failure: archive raw bytes, insert `malformed_payloads` record with error metadata, return 202 (ack, not reject). Malformed count exposed as Prometheus metric. Payload never discarded — always in raw archive. **Idempotency note:** Malformed payloads share idempotency key with their parsed counterparts. If key is already seen, return 202 without re-archiving.
**Verify:** Send non-JSON → 202, raw bytes archived, malformed metric incremented. Send valid JSON with missing required fields → same.
**Fix:** Error message does not leak payload content to response.
**Scale:** Malformed rate alert threshold configurable.

### M1-15: C1 Queue depth limiting
**Requirements:** ING-08
**Build:** Check queue depth before enqueue. If depth > `max_queue_depth`: return 429 with `Retry-After` header set to estimated drain time (queue depth / worker rate). Never block the HTTP handler. Queue depth configurable: `SPINDLE_INGEST_MAX_QUEUE_DEPTH` (default: 100,000 — ~11 minutes at 150/s).
**Verify:** Fill queue to limit → next request 429. Queue drains → next request 202. No data loss during saturation.
**Fix:** 429 response includes current queue depth for debugging.
**Scale:** Adaptive retry-after based on drain rate.

### M1-16: C1 Rate limiting
**Requirements:** ING-09
**Build:** Token-bucket rate limiter via `governor` crate. Configurable `SPINDLE_INGEST_RATE_LIMIT` (runs/sec, default: 500). Per-deployment (not per-node — single-tenant). **Multi-tenant note:** M2 will add per-tenant rate limiting. M1 assumes single-tenant. Exceeded → 429 with `Retry-After`. Rate limit metrics: `spindle_ingest_rate_limit_hits_total`.
**Verify:** Burst above limit → 429s. Steady state below limit → all 202. Reset after cooldown.
**Fix:** Burst allowance tuned to absorb converge storms.
**Scale:** Make rate limit a hot-reloadable config.

### M1-17: C1 InSpec direct ingest
**Requirements:** ING-10
**Build:** `POST /ingest/events/inspec` — same auth, same archive-before-parse pattern. Accepts InSpec JSON reporter output format. Shares idempotency, rate limiting, and malformed handling with data-collector endpoint. Differentiate in metrics by `source=inspec` label.
**Verify:** Post InSpec JSON → 202 → control results in DB. Duplicate → idempotent.
**Fix:** InSpec JSON schema variations handled (different reporter versions).
**Scale:** N/A — same scaling as data-collector.

### M1-18: C1 Payload size limiting
**Requirements:** ING-11
**Build:** `axum::extract::DefaultBodyLimit` with configurable `SPINDLE_INGEST_MAX_PAYLOAD_SIZE` (default: 32MB). Exceeded → 413 with message indicating max. Validate default against corpus: largest payload in captured traffic.
**Verify:** Post 33MB → 413. Post 31MB → 202. Config change takes effect on restart.
**Fix:** Error response includes actual vs max sizes.
**Scale:** Streaming body reader to avoid buffering entire payload.

### M1-19: C1 Horizontal scalability
**Requirements:** ING-12
**Build:** Verify: no in-memory state between requests. Idempotency cache backed by Postgres (not in-memory). Queue in Postgres (shared). Token config from config file (consistent across instances). Test: two server instances behind round-robin, ingest round-robin, verify idempotency holds.

**Test procedure:** 1) Deploy two `spindle-server` instances with shared DB. 2) Send payload to instance A → 202. 3) Send same payload to instance B → 202 (idempotent). 4) Query DB — both attempts logged, single row inserted. 5) Queue depth consistent across instances.

**Verify:** Two instances → duplicate payload arrives at different instances → second is idempotent. Queue depth consistent across instances.
**Fix:** Any accidentally per-instance state moved to shared store.
**Scale:** Document load-balancer configuration.

### M1-20: C3 Pipeline — parse + normalize
**Requirements:** PIPE-01
**Build:** `spindle-pipeline` worker: dequeue job, fetch raw payload from archive, parse JSON into internal structs. Three parsers: `RunStartParser`, `RunConvergeParser`, `ComplianceReportParser`. Normalize: timestamps to UTC, string statuses to enum variants, nested JSON to flat structs. Extract node identity from payload, upsert node record. Insert run record. Iterate resources.
**Verify:** Corpus payload → correct run row, correct node row, correct resource event count (including no-ops before filter stage).
**Fix:** Field name variations from different Chef versions mapped.
**Scale:** Parser selection by payload fingerprint, not content-type.

### M1-21: C3 No-op filtering with status counts
**Requirements:** PIPE-02, PIPE-03
**Build:** After PIPE-01 normalization: for each resource event, if status is `up-to-date`: increment run's `total_resource_count` only, do NOT insert into `resource_events`. If status is `updated`, `failed`, or `skipped`: insert into `resource_events` AND increment run's status-specific count. Store run's `updated_count`, `failed_count`, `skipped_count` from PIPE-03.
**Verify:** Corpus with 100 events (95 up-to-date, 3 updated, 2 failed) → `resource_events` table has 5 rows, run has `total=100, updated=3, failed=2, skipped=0`.
**Fix:** Count reconciliation: `updated + failed + skipped + (total - persisted) = total`.
**Scale:** Batch insert for remaining rows to minimize round-trips.

### M1-22: C3 Duration rollups
**Requirements:** PIPE-04
**Build:** Even for filtered (up-to-date) events, extract `duration_ms`. Every 15 minutes or on batch flush: INSERT INTO `duration_rollups` with hour-truncated timestamp, aggregating count, sum, and streaming percentile approximations using the `tdigest` crate for p50/p95/p99/max. T-Digest accuracy: ±5% within 1% of exact for p99 (validate on small test set). Key: `(hour, cookbook_name, cookbook_version, resource_type, platform)`.
**Verify:** Known durations → rollup query returns correct p95 within ±2ms. Rollup covers all events (including filtered ones) — verify total count matches.
**Fix:** Percentile accuracy validated against exact computation on small test set.
**Scale:** Rollup merge window configurable. Batch size controls memory.

### M1-23: C3 Control result pass-through
**Requirements:** PIPE-05
**Build:** Control results from compliance_report payloads are NEVER filtered. Every single one is inserted into `control_results` table. Comment in code: "// PIPE-05: control results are never filtered. Do not add filtering here." Test enforcement: compile-time assertion that control_result processing path has no filter.
**Verify:** Corpus with 400 controls → 400 rows in control_results. Filter module has no code path touching control results.
**Fix:** Architectural enforcement: control results go through separate insert path from resource events.
**Scale:** Batch insert for control results (400/scan × 20k nodes = 8M/day).

### M1-24: C3 Unknown field preservation
**Requirements:** PIPE-06
**Build:** Parse payloads with `#[serde(flatten)]` or `serde_json::Value` for unrecognized top-level and nested fields. Store in `extra_fields JSONB` column on each table. This ensures future Chef versions adding fields don't lose data.
**Verify:** Payload with field not in schema → field present in `extra_fields` JSONB, retrievable via query.
**Fix:** Schema evolution doc: how to promote extra fields to columns.
**Scale:** Extra fields indexed via GIN for ad-hoc queries.

### M1-25: C3 Dead-letter queue
**Requirements:** PIPE-07
**Build:** Processing failure (panic, unrecoverable parse error, DB constraint violation after retries): move job to `pipeline_dead_letter` table with original archive reference, error message, stack trace, timestamp, retry count. Increment `spindle_pipeline_dead_letter_total` metric with labels for error type. Admin endpoint: `GET /v1/admin/dead-letter` (list), `POST /v1/admin/dead-letter/{id}/retry`. **Retry semantics:** Re-enqueue to processing queue. If still malformed, increment `spindle_pipeline_dead_letter_total{error_type=malformed}` and move to permanent dead letter (no further auto-retry).
**Verify:** Deliberately malformed payload in corpus → processed, fails, lands in dead letter. Retry → if still fails, error count increments. Admin endpoint lists it.
**Fix:** Dead letter retention: 30 days, then archive and drop.
**Scale:** Dead letter listing paginated.

### M1-26: C3 Schema version stamping + cookbook usage
**Requirements:** PIPE-08, PIPE-09
**Build:** `schema_version` column on every derived table (INT, starts at 1). Incremented when schema changes; migration adds new version. **Version increment rules:** New version on: (1) table added, (2) column added/removed, (3) column type changed. No increment for: index changes, partition changes, or migration-only additions. `cookbook_usage` table populated during run processing: extract `cookbook_name`, `cookbook_version` from resource events, upsert into `cookbook_usage` with `node_id`, `run_id`, `first_seen`, `last_seen`.
**Verify:** Schema version = 1 on all rows. Cookbook usage table populated after corpus processing.
**Fix:** Deduplication of cookbook_usage rows (per run, per node).
**Scale:** Cookbook inventory query performance verified.

---

## M2 — Query + Authorization (14 tasks)

### M2-01: C5 Filter grammar crate
**Requirements:** API-03
**Build:** `spindle-api::filter` module defining shared filter grammar: `Filter { field: String, operator: FilterOp, value: Value }`. Operators: `eq`, `neq`, `gt`, `gte`, `lt`, `lte`, `in`, `like`, `between`, `is_null`. Time range: `start_time`, `end_time` (RFC 3339). Sort: `Sort { field: String, direction: Asc|Desc }`. Parser from query string: `?filter[platform]=ubuntu&sort=start_time:desc&since=2026-01-01T00:00:00Z`. Validation: unknown field → 400 with list of valid fields.
**Verify:** Parse valid filter → correct struct. Parse unknown field → 400. Every list endpoint uses this module.
**Fix:** Reserved characters handled (%2C for comma, etc.).
**Scale:** Filter compilation to SQL via sqlx prepared statements.

### M2-02: C5 Pagination
**Requirements:** API-04
**Build:** Cursor pagination: `?cursor=<opaque>&limit=100` (max 1000). Cursor encodes: `(sort_field_value, id, direction)`, base64-encoded, not meaningful to clients. Deterministic ordering via `ORDER BY sort_field, id` (id breaks ties). Response includes `next_cursor` (null if last page), `total_count` (scoped), `has_more: bool`.
**Verify:** Paginate through 1000 rows → no duplicates, no skips. Insert row mid-pagination → cursor still returns all existing rows (snapshot isolation). Last page → next_cursor null.
**Fix:** Cursor encoding/decoding failures → 400 with clear message.
**Scale:** Keyset pagination (not offset) avoids performance degradation on deep pages.

### M2-03: C5 Nodes endpoint
**Requirements:** API-02
**Build:** `GET /v1/nodes` — filter by platform, environment, policy_group, name, last_seen range. `GET /v1/nodes/{id}` — full detail including current attributes. `GET /v1/nodes/{id}/state` — lean current state (no attribute history). All support same filter/sort/pagination grammar. `NodeResponse` struct with all fields.
**Verify:** Filter by platform → correct nodes. Pagination works. Scoped to project → only project nodes returned.
**Fix:** Attribute JSONB querying performance (uses expression indexes).
**Scale:** Large attribute JSONB (5+ MB) handled gracefully.

### M2-04: C5 Runs endpoint
**Requirements:** API-02
**Build:** `GET /v1/runs` — filter by node_id, status, start_time range, cookbook. `GET /v1/runs/{id}` — full detail with resource events (paginated sub-list). Resource events detail includes duration, delta, guard outcome.
**Verify:** Filter by node+time → correct run. Detail includes all resource events (paginated). Run with 2000 events → pagination handles it.
**Fix:** Resource event sub-pagination uses same cursor grammar.
**Scale:** Run detail without N+1 queries.

### M2-05: C5 Resource event aggregates
**Requirements:** API-02
**Build:** `GET /v1/resource-events/aggregates` — group by cookbook (+version), resource_type, platform. Metrics: count, sum_duration, avg_duration, p50, p95, p99, max (from rollup table via PIPE-04). `GET /v1/resource-events/drift` — resources by update frequency over configurable window (identifies "converging repeatedly" pattern). Both use filter grammar.
**Verify:** Aggregate query returns correct percentiles (within 5% of exact). Drift query identifies resources with >90% update rate.
**Fix:** Aggregate granularity: hourly default, sub-hour via query param.
**Scale:** Aggregate queries hit rollup table (M rows, not B rows).

### M2-06: C5 Compliance endpoints
**Requirements:** API-02
**Build:** `GET /v1/compliance/reports` — filter by node, profile, time range, status. `GET /v1/compliance/reports/{id}` — full detail with control results. `GET /v1/compliance/controls` — filter by control_id, status, impact. `GET /v1/compliance/nodes/{id}/status` — per-node compliance summary. `GET /v1/compliance/profiles/{id}/status` — per-profile summary.
**Verify:** Filter by control status=failed → correct results. Node status endpoint returns last scan summary. Scoped auditor → no node attributes leaked.
**Fix:** Large control result sets paginated.
**Scale:** Status rollups pre-computed for fast summaries.

### M2-07: C5 Waivers CRUD
**Requirements:** API-02
**Build:** `POST/GET/PUT/DELETE /v1/waivers`. Waiver schema: control_id, scope (node/project/global), justification, approver, start_date, expiry_date. Waived controls excluded from compliance status calculations with waiver reference. Expired waivers automatically excluded.
**Verify:** Create waiver → control marked waived. Expired waiver → control reverts to real status. DELETE waiver → same.
**Fix:** Waiver audit log: every CRUD event recorded.
**Scale:** Waiver evaluation cached per scan.

### M2-08: C5 Cookbook inventory + health
**Requirements:** API-02
**Build:** `GET /v1/cookbooks` — which versions running where, last seen, node count per version. `GET /v1/health` — ingest lag (queue depth, oldest unprocessed), API version, DB connectivity, storage connectivity, Dex health. `GET /v1/health/metrics` — Prometheus endpoint.
**Verify:** Cookbook inventory reflects processed runs. Health endpoint returns 200 when all subsystems up, 503 when DB down.
**Fix:** Health check timeout: 5s per subsystem, parallel.
**Scale:** Health endpoint cached for 5s to avoid thundering herd.

### M2-09: C5 OpenAPI generation
**Requirements:** API-06
**Build:** `utoipa` crate annotations on all handlers, request/response types, filter params. OpenAPI JSON at `GET /v1/openapi.json`. UI at `GET /v1/docs` (Swagger UI or Scalar). Generated from code — not hand-maintained.
**Verify:** OpenAPI doc served, valid JSON, all endpoints present. Contract tests generated from spec pass.
**Fix:** Missing schemas added. Ambiguous types disambiguated.
**Scale:** OpenAPI generation in CI, diff against committed version.

### M2-10: C5 Uniform error envelope
**Requirements:** API-07
**Build:** Ensure every response (success and error) carries `request_id` header and `{ "api_version": "v1", "request_id": "..." }` in body. Error envelope: `{ "error": { "code": "INVALID_FILTER", "message": "...", "details": {...} }, ... }`. Success envelope for list endpoints: `{ "data": [...], "pagination": {...}, ... }`.
**Verify:** 200 response has version+request_id. 400 has error envelope. 500 has error envelope (never raw stack trace). request_id matches X-Request-Id header.
**Fix:** Any endpoint missing envelope → middleware enforces.
**Scale:** Envelope deserialization test suite shared across all endpoints.

### M2-11: C5 Data provenance markers
**Requirements:** API-08
**Build:** Every response: `api_version: "v1"`. Where data is derived from rollups: `provenance: { "source": "rollup", "granularity": "hourly" }`. Where response includes node attributes: only `compliance-auditor` role receives `stripped_attributes: true` marker. Structured, not prose.
**Verify:** Run detail → provenance absent (direct data). Aggregate query → provenance marks rollup source.
**Fix:** Provenance markers on all appropriate endpoints.
**Scale:** Provenance chain extends to restored archive data (v1.1).

### M2-12: C8 Authorization — query-layer scoping
**Requirements:** AUTHZ-01, AUTHZ-02
**Build:** `spindle-authz::Scope` struct: `projects: HashSet<String>`, `roles: HashSet<Role>`. Every store method requires `&Scope` parameter — enforced at compile time (no default, no Option). `ScopeFilter` trait implemented by each entity: translates scope to SQL WHERE clause appending project/role filters. `compliance-auditor` role: node attributes stripped from responses at the store layer before serialization.
**Verify:** Compile test: calling `get_run(pool, id)` without scope → compiler error. Auditor token → `GET /v1/nodes` → attributes null/absent. Auditor getting node counts → only own projects.
**Fix:** Scope applied to COUNT queries, aggregates, existence checks — not just data retrieval.
**Scale:** Scope evaluation performance: single JOIN, indexed.

### M2-13: C8 Authorization — role model + enforcement
**Requirements:** AUTHZ-03, AUTHZ-04, AUTHZ-05
**Build:** `Role` enum: `Ingest`, `Viewer`, `ComplianceAuditor`, `TokenAdmin`, `Admin`. `Ingest`: write-only access to ingest endpoints. `Viewer`: read all non-compliance data. `ComplianceAuditor`: compliance read + export, no node attributes. `TokenAdmin`: manage tokens. `Admin`: all. Each endpoint annotated with `#[require_role(Ingest)]` or similar. Store methods gated by role checks in addition to scope. Every decision: `audit_log INSERT (subject, resource, decision, rule)`.
**Verify:** Ingest token → GET /v1/nodes → 403. Viewer token → POST /v1/ingest → 403. Auditor token → GET /v1/compliance/reports → 200, node attributes stripped.
**Fix:** Role hierarchy: Admin includes all.
**Scale:** Role checks cached per request (not per query).

### M2-14: C8 Negative-authorization suite
**Requirements:** AUTHZ-06
**Build:** Test suite: for every endpoint (18+), for every non-admin role, assert:
1. Can access allowed endpoints → 200
2. Cannot access denied endpoints → 403
3. Scoped to project A → cannot see project B nodes in list, detail, count, or aggregate
4. Auditor → node attributes stripped on every endpoint that could leak them
5. Pagination totals respect scope (no count leakage)
Run as integration test in CI. One code path — same middleware for session + token (see M3).
**Verify:** All negative tests pass. No endpoint missing from test coverage (checked by endpoint enumeration).
**Fix:** Add test for every new endpoint (CI gate).
**Scale:** Parameterized test generation from OpenAPI spec.

---

## M3 — Identity (14 tasks)

### M3-01: C6 Dex deployment + config generation
**Requirements:** ADR-05 (confirmed), IDP-01
**Build:** Dex binary bundled with Spindle release (sidecar model). `spindle-server` generates `dex.config.yaml` from Spindle config, writes to runtime directory, starts Dex as child process (or operator runs separately). Health check: poll Dex `/.well-known/openid-configuration` until ready, then start serving.
**Verify:** Dex starts → discovery doc returns 200. Connectors configured from Spindle config → Dex lists them.
**Fix:** Dex process lifecycle: graceful shutdown via SIGTERM propagated.
**Scale:** Document sidecar vs. external Dex deployment.

### M3-02: C6 Internal identity model + principal
**Requirements:** IDP-01
**Build:** Implement the M0-09 interface. `Principal` struct with Dex connector integration: after Dex authenticates, Dex returns id_token; Spindle validates, extracts claims, resolves groups via Dex API or direct LDAP. Groups cached with configurable TTL. `InternalRoles` derived from group→role mapping rules.
**Verify:** Auth flow: Dex callback → Principal populated → roles resolved → session created.
**Fix:** Group resolution timeout (LDAP slow path) handled gracefully.
**Scale:** Group cache in Redis (v1: in-memory with 5min TTL).

### M3-03: C6 OIDC connector
**Requirements:** IDP-02
**Build:** OIDC flow via Dex: `GET /v1/auth/login?connector=oidc` → redirect to Dex → callback → `GET /v1/auth/callback` → validate state/nonce → exchange code → validate id_token → create session. PKCE enforced. Tested against Keycloak (CI) + Okta/Entra ID (manual). `openidconnect` crate for discovery and token validation.
**Verify:** Full OIDC flow in CI (Keycloak container). Invalid state → 401. Expired token → 401. PKCE mismatch → 401.
**Fix:** Redirect URI validation against configured list.
**Scale:** OIDC discovery cached with TTL.

### M3-04: C6 SAML connector
**Requirements:** IDP-03
**Build:** SAML 2.0 via Dex: SP-initiated (user clicks login → redirect to IdP → SAML response → validate assertion → session) and IdP-initiated (IdP POSTs assertion → validate → session). Signed assertions validated against IdP public key. Encrypted assertions decrypted with SP private key. Metadata exchange: serve SP metadata at `/v1/auth/saml/metadata`.
**Verify:** SP-initiated flow against Keycloak SAML. IdP-initiated POST accepted. Unsigned assertion → rejected. Expired assertion → rejected.
**Fix:** SAML library limitations documented. Certificate rotation procedure.
**Scale:** Metadata caching with configurable refresh.

### M3-05: C6 LDAP/AD connector
**Requirements:** IDP-04
**Status:** ✅ Complete
**Build:** LDAP bind via Dex: user DN resolution (configurable base DN + filter), direct bind for password validation. Nested group resolution: recursive group membership query with configurable depth limit. Referral handling: follow or reject (configurable). Group cache with configurable TTL (default: 15min), manual refresh endpoint for admins.
**Verify:** Bind against OpenLDAP in CI → success. Bad password → failure. Nested group → all parent groups resolved. Cache: modify group → cache hit returns old → TTL expires → new.
**Fix:** TLS required for production LDAP (StartTLS or LDAPS). Connection pooling.
**Scale:** LDAP connection pool with health checks.
**Implementation:** `spindle-dex/src/ldap_connector.rs` module with `LdapConnector`, `LdapConnectorConfig`, `LdapOperations` trait (for testable mock LDAP), `LdapAuthenticator` trait, `LdapAuthResult`, `LdapError`. Key features: (1) User DN resolution via configurable search filter with `{user}` placeholder, (2) Direct bind with resolved DN + password for authentication, (3) Nested group resolution via recursive membership queries with configurable depth limit (default 5), (4) Referral handling — `follow_referrals` config flag, referrals rejected by default, (5) Group cache with configurable TTL (default 900s/15min), per-principal caching keyed by `connector:subject`, cache expiry + manual refresh via `refresh_groups()`, (6) TLS enforcement — `require_tls` config, non-local servers require TLS, (7) Connection pooling via `pool_size` config + `set_conn_timeout`. 254 tests passing (41 integration + 33 unit in spindle-dex, 210 in spindle-server, 45 negative_auth).

### M3-06: C6 Local accounts
**Requirements:** IDP-05
**Build:** Local user store in Postgres. Registration disabled by default (config: `SPINDLE_LOCAL_ACCOUNTS_ENABLED=false`). When enabled: `POST /v1/auth/local/register` (admin-only or bootstrap mode). Argon2id password hashing with configurable parameters. Forced rotation: configurable max age, warning period, enforcement. Audit log: every login attempt (success/failure), password change, account lock.
**Verify:** Air-gapped: start with all external connectors unreachable → local login succeeds. Password < 12 chars → rejected. After 5 failed attempts → account locked (configurable).
**Fix:** Bootstrap admin account creation on first startup.
**Scale:** Rate limiting on login attempts.

### M3-07: C6 Multiple connectors + JIT provisioning
**Requirements:** IDP-06, IDP-08
**Build:** Connector selection: user picks from enabled connectors at login (dropdown in UI, `?connector=` in API). Multiple connectors active simultaneously: Okta + LDAP service accounts, etc. JIT: on first successful login from any connector, create user record with `subject` + `connector` as unique key. Provision internal roles from group mappings.
**Verify:** Login via OIDC → user created. Login via LDAP → different connector, same pattern. Same subject on different connector → separate user records (or merged if email matches).
**Fix:** User deduplication strategy documented.
**Scale:** JIT provisioning as transaction (avoid partial user creation).

### M3-08: C6 Group/claim mapping rules
**Requirements:** IDP-07
**Status:** ✅ Complete
**Build:** Mapping config: `[[identity.mappings]]` array in config. Each rule: `connector`, `match_type: group|claim`, `match_value: regex`, `assign_roles: [...]`, `assign_scope: [...]`. Deterministic precedence: first match wins, rules evaluated in config file order. Documented, not configurable precedence. `spindle config validate` checks for ambiguous rules.
**Verify:** Two matching rules → first one applies. Non-matching rule → skipped. Rule order change → different outcome (documented behavior).
**Fix:** Circular group references detected and rejected.
**Scale:** Rule evaluation cached per principal.
**Implementation:** `spindle-config/src/mappings.rs` module with `MappingRule`, `MatchType`, `MappingResult`, `MappingEvaluator`, `validate_mappings()`. Validation detects: invalid regex, empty match_value, missing claim_key for claim rules, ambiguous/superset regex conflicts, and circular group references via DFS. Evaluator uses first-match-wins, config-order evaluation, and per-principal caching (keyed by `connector:subject`). 78 tests passing.

### M3-09: C6 Mapping preview endpoint
**Requirements:** IDP-09
**Build:** `POST /v1/auth/mappings/preview` (admin): `{ "connector": "ldap", "claims": {"groups": ["engineering", "us-east"]} }` → `{ "roles": ["viewer"], "scope": ["project-engineering", "project-us-east"] }`. Evaluates mapping rules without creating a user or requiring a login. Admin-only endpoint.
**Verify:** Test matrix of claims → correct roles predicted. Missing group → no role assigned. Empty claims → empty roles.
**Fix:** Error on malformed claims input → 400 with field names.
**Scale:** Used by support, not hot path — no caching needed.

### M3-10: C6 Session management
**Requirements:** IDP-11
**Status:** ✅ Complete
**Build:** JWT access token (short-lived, default 15min) + refresh token (longer, default 8h). Stored in `sessions` table. Configurable idle timeout (default: 30min) and absolute timeout (default: 12h). Single-logout: where IdP supports OIDC RP-Initiated Logout or SAML SLO, propagate. Admin revocation: `DELETE /v1/admin/sessions/{id}` (individual), `DELETE /v1/admin/sessions?user_id=X` (bulk).
**Verify:** Test matrix of claims → correct roles predicted. Missing group → no role assigned. Empty claims → empty roles. Error on malformed claims input → 400 with field names. Used by support, not hot path — no caching needed.
**Fix:** JWT access token (15min default) with `SessionClaims` struct (sub, session_id, connector, token_type, iat, exp, scope, iss). Refresh token (8h default) with rotation. Configurable idle timeout (30min default) + absolute timeout (12h default). Admin revocation: `revoke_session(id)` + `revoke_user_sessions(user_id)`. Refresh token rotation: one-time use, new refresh token on each refresh. Session cleanup job via `cleanup_expired()`. Implemented in `spindle-server/src/sessions.rs` with `SessionManager`, `SessionStore` trait, `InMemorySessionStore`, `LdapAuthResult`-like `SessionRecord`. Token hash stored with SHA-256. JWT via `jsonwebtoken` crate (HS256). 320 tests passing (24 session unit + 234 lib + 41 LDAP integration + 45 negative auth).
**Verify:** Access token expires → refresh → new access token. Idle timeout → 401. Admin revoke → next request 401.
**Fix:** Refresh token rotation: one-time use, new refresh token on each refresh.
**Scale:** Session cleanup job for expired tokens.

### M3-11: C7 Token types + creation
**Requirements:** TOK-01, TOK-02, TOK-03
**Status:** ✅ Complete
**Build:** `POST /v1/tokens` → create token: name, description, owner (user or service account), type (user/service/agent), role selection (≤ owner roles), scope selection (≤ owner scope), TTL (≤ policy max). Response: `{ "id": "...", "name": "...", "token": "sp_xxxx...xxxx" }` — plaintext shown once. Store: Argon2id hash, never retrievable. `GET /v1/tokens` → list tokens (no plaintext, just metadata). Policy max TTL: configurable per token type. Token prefix `sp_` for easy identification in logs/audit.
**Verify:** Create token → plaintext returned → GET /v1/tokens → no plaintext. Create user token with role exceeding owner → 403. Agent token default TTL=1h.
**Fix:** Token prefix `sp_` for easy identification in logs/audit. Argon2id hash stored with `password-hash` crate. Token validation checks revoked + expiry. Audit logged.
**Scale:** Token creation audit logged. 
**Implementation:** `spindle-server/src/tokens.rs` module with `TokenType` enum (User/Service/Agent), `CreateTokenRequest`, `TokenMetadata`, `TokenCreateResponse`, `TokenError`, `TokenPolicy` (max TTL: user=30d, service=365d, agent=1h), `OwnerInfo` for role/scope validation, `TokenManager` (create/validate/revoke/list), `TokenStore` trait + `InMemoryTokenStore`. Token generation with `sp_` prefix + UUIDv4. Argon2id hashing via `argon2` 0.5 crate. Role validation: requested roles/scopes must be subset of owner's. TTL validation: must be ≤ policy max for token type. 42 tests passing (28 token + 234 lib + 41 LDAP + 45 negative auth). 351 tests green.

### M3-12: C7 Token lifecycle
**Requirements:** TOK-04, TOK-05, TOK-06, TOK-07
**Status:** ✅ Complete
**Build:** `DELETE /v1/tokens/{id}` — single revocation, takes effect next request. `DELETE /v1/tokens?owner=X` — bulk revocation (single UPDATE). `DELETE /v1/tokens?scope=Y` — bulk by scope. Expiry: auto via timestamp; warning emails at T-7d and T-1d via `tokens_expiring_within()`. Rotation: `rotate_token()` creates new token with overlapping validity → client rotates → revokes old. `last_used_at` updated per request (sampled: max once/5min to reduce DB writes).
**Verify:** revoke → immediate 401. Rotate → both work during overlap, old fails after revocation. Bulk revoke → all affected tokens 401.
**Fix:** Bulk revocation uses single pass on in-memory store (single lock + iterate). Expiry cleanup via `cleanup_expired_tokens()`. Sampled last_used_at prevents excessive DB writes.
**Scale:** Token creation audit logged. 10 new lifecycle tests added (376 tests total green).
**Implementation:** Added to `spindle-server/src/tokens.rs`: `revoke_tokens_by_owner()` (bulk by owner, single pass), `revoke_tokens_by_scope()` (bulk by scope prefix match), `rotate_token()` (create new + revoke old), `tokens_expiring_within(warn_secs)` (for T-7d/T-1d warnings), `cleanup_expired_tokens()` (auto-revoke expired). `update_last_used()` now samples: only writes if ≥5min since last update. `TokenStore` trait extended with 3 new methods. `InMemoryTokenStore` implements all. 10 new tests: bulk revoke by owner, bulk revoke by scope, expiry warning (7d/1d), token rotation, rotation preserves roles/scopes, rotate nonexistent token → NotFound, cleanup expired tokens, revoke → AlreadyRevoked error on validate, bulk revoke skips already-revoked, T-7d/T-1d warning matrix.

### M3-13: C7 Idle token report + audit
**Requirements:** TOK-09
**Status:** ✅ Complete
**Build:** `get_token_with_last_used(id)` returns metadata + last_used_at. Idle report: tokens not used for N days (configurable, default 90d), `GET /v1/admin/tokens/idle?since_days=N`. Audit log: every token create/revoke/rotate/disable/enable event with timestamp, actor, and target token metadata. 1 new audit test.
**Verify:** Create token → never used → appears in idle report after 90d. Use token → last_used_at updated (sampled) → disappears from idle report.
**Fix:** Idle report excludes revoked and disabled tokens. Audit log records token ID + name + owner (never plaintext).
**Scale:** Audit log retention in `audit_log` table (1 year). Idle report query uses indexed last_used_at.
**Implementation:** Added `get_token_with_last_used()` to `TokenStore` trait + `InMemoryTokenStore`. `tokens_idle_since()` for idle token listing. Audit log via `tracing` macros. 2 new tests. 380 tests green.

### M3-14: C7 Reconciliation job
**Requirements:** TOK-08
**Status:** ✅ Complete
**Build:** Reconciliation job: for each user-owned token (not service accounts), resolve owner against source connector via `UserResolver` trait. Connector unreachable → skip (don't disable on transient failures). Owner no longer resolvable → disable token (separate `disabled` flag, not `revoked`), log to audit, add to orphan report. `GET /v1/admin/tokens/orphans` → admin view via `list_disabled_tokens()`. Idempotent — running twice doesn't double-disable (disabled tokens excluded from reconciliation set via `list_tokens_for_reconciliation()`). Batch resolution per connector to minimize LDAP queries. `enable_token()` for manual reactivation (no auto-renable).
**Verify:** User removed from LDAP → reconciliation → token disabled → orphan report shows it → 401 on use. User re-added → reconciliation detects but NOT auto-renable (manual admin action required).
**Fix:** `list_tokens_for_reconciliation()` filters: User type only, not revoked, not disabled. Batch by connector via HashMap. Connector unavailable → `ReconciliationError::ConnectorUnavailable` → skip. `validate_token()` returns `TokenError::TokenDisabled` for disabled tokens.
**Scale:** Reconciliation runs as periodic `spindle-worker` task (configurable interval, default 1h). `ReconciliationResult` tracks checked/disabled/skipped/orphaned_ids. `UserResolver` trait allows real LDAP/IdP integration.
**Implementation:** Added `TokenDisabled` error variant, `disabled`/`disabled_reason`/`connector` fields to `TokenMetadata`, `UserResolver` trait + `ReconciliationError`/`ReconciliationResult` types, `reconcile_tokens()` method to `TokenManager`. Extended `TokenStore` trait with `disable_token()`, `enable_token()`, `list_disabled_tokens()`, `list_tokens_for_reconciliation()`. `InMemoryTokenStore` implements all. 14 new reconciliation tests with MockResolver. 394 tests green.
**Implementation:** Added to `spindle-server/src/tokens.rs`: `revoke_tokens_by_owner()` (bulk by owner, single pass), `revoke_tokens_by_scope()` (bulk by scope prefix match), `rotate_token()` (create new + revoke old), `tokens_expiring_within(warn_secs)` (for T-7d/T-1d warnings), `cleanup_expired_tokens()` (auto-revoke expired). `update_last_used()` now samples: only writes if ≥5min since last update. `TokenStore` trait extended with 3 new methods. `InMemoryTokenStore` implements all. 10 new tests: bulk revoke by owner, bulk revoke by scope, expiry warning (7d/1d), token rotation, rotation preserves roles/scopes, rotate nonexistent token → NotFound, cleanup expired tokens, revoke → AlreadyRevoked error on validate, bulk revoke skips already-revoked, T-7d/T-1d warning matrix.
**Build:** Revocation: `DELETE /v1/tokens/{id}` (individual), `DELETE /v1/tokens?owner=X` (bulk), `DELETE /v1/tokens?scope=Y` (bulk). Takes effect on next request (checked per-request against revocation table). Expiry: automatic via expiry timestamp; `spindle-worker` sends warning at T-7d, T-1d to owner email (if configured) and admin report. Rotation: create new token with overlapping validity → rotate client → revoke old. `last_used_at` updated on each authenticated request (sampled: max once per 5min per token to reduce DB writes).
**Verify:** Revoke token → immediate next request 401. Rotate → new token works, old token works during overlap, old token fails after revocation. `last_used_at` updates.
**Fix:** Bulk revocation performance (single UPDATE WHERE, not N DELETEs).
**Scale:** Token usage sampling to avoid write amplification.

### M3-13: C7 Idle token report + audit
**Requirements:** TOK-07, TOK-09
**Build:** `GET /v1/admin/tokens/idle?days=90` → list tokens unused for ≥ N days, with owner info, creation date, last_used_at. `GET /v1/admin/tokens/audit?token_id=X&from=&to=` → audit trail of token usage (sampled, see TOK-07). Lifecycle events: create, rotate, revoke, expire, orphan-disable logged to `audit_log` table.
**Verify:** Token unused 100 days → appears in idle report. Audit log shows creation event + revocation event.
**Fix:** Idle report includes suggestion to revoke.
**Scale:** Token audit log retention = session retention policy.

### M3-14: C7 Reconciliation job
**Requirements:** TOK-08
**Build:** `spindle-worker` periodic job (configurable interval, default 1h): for each token owned by a user (not service account), resolve owner against their source connector. If connector unavailable → skip (don't disable on transient failures). If owner no longer resolvable → disable token, log to `audit_log`, add to orphan report. `GET /v1/admin/tokens/orphans` → admin report of all disabled-by-reconciliation tokens.
**Verify:** User removed from LDAP → reconciliation runs → token disabled → appears on orphan report → attempt to use token → 401. User added back → reconciliation detects (manual reactivation by admin required).
**Fix:** Reconciliation job idempotent — running multiple times doesn't double-disable.
**Scale:** Batch resolution of owners by connector to minimize LDAP queries.

---

## M4 — Evidence (16 tasks)

### M4-01: C9 Signer interface + local implementation
**Requirements:** SIG-01, SIG-08
**Build:** `spindle-signing::Signer` trait: `sign(data) -> Signature`, `public_key() -> PublicKey`, `key_id() -> KeyId`. Local implementation: Ed25519 keypair generated at install (`spindle key generate`), stored encrypted at rest (AES-256-GCM, key derived from unlock material: file path, env var `SPINDLE_KEY_UNLOCK`, or operator prompt). Unlock at startup. Full air-gap: no external call.
**Verify:** Generate key → sign data → verify with public key. Restart without unlock → startup fails with clear message. Wrong unlock material → startup fails.
**Fix:** Key file permissions: 0600. Unlock material audit: never logged.
**Scale:** Key rotation creates new key, retains old (SIG-03).

### M4-02: C9 External signer — PKCS#11
**Requirements:** SIG-01
**Build:** PKCS#11 implementation using `cryptoki` crate. Config: `[signing.external.pkcs11] module_path, slot_id, key_label, pin (env var)`. `sign()` → PKCS#11 C_Sign via cryptoki. Key never enters process memory. `key_id()` → PKCS#11 CKA_ID attribute. `public_key()` → PKCS#11 C_GetAttributeValue CKA_PUBLIC_EXPONENT + CKA_MODULUS.
**Verify:** CI: test against SoftHSM2 container. Sign → SoftHSM performs operation → signature returned. PIN wrong → clear error. Slot empty → clear error.
**Fix:** PKCS#11 session management: pool of sessions, reconnect on disconnect.
**Scale:** Pin caching: PIN required only at startup, not per-signature.

### M4-03: C9 External signer — KMS
**Requirements:** SIG-01
**Build:** AWS KMS implementation using `aws-sdk-kms` (behind feature flag). Config: `[signing.external.aws_kms] key_id, region`. `sign()` → AWS KMS Sign API. Azure Key Vault, GCP KMS behind respective feature flags. Common `ExternalSigner` trait wrapping provider-specific clients. Key identifier: KMS key ARN.
**Verify:** CI: test with localstack KMS. Sign → localstack performs operation → signature returned.
**Fix:** Credential chain: env vars → instance profile → config file.
**Scale:** KMS client with connection pooling and retry.

### M4-04: C9 Key identifier recording
**Requirements:** SIG-02
**Build:** Every manifest, export, and checkpoint stores `signing_key_id: String`. Inserted into manifest JSON, Parquet footer metadata, checkpoint row. Database column: `signatures.key_id` on manifests table. Enforcement: `Signer` trait required for manifest/export creation — compile-time impossible to create unsigned artifact.
**Verify:** Manifest in DB → key_id present. Export manifest → key_id present. Key rotation → new artifacts show new key_id, old ones retain old key_id.
**Fix:** Key ID format: `local:<sha256_of_public_key>` or `aws-kms:<key_arn>`.
**Scale:** Key ID indexed for audit queries.

### M4-05: C9 Historical key retention + rotation
**Requirements:** SIG-03, SIG-04
**Build:** `public_keys` table: `key_id TEXT PK`, `public_key BYTEA`, `created_at`, `retired_at` (nullable). On rotation: new key added with `created_at`, old key `retired_at` set (still retained). Verification: `verify(signature, data, key_id)` → look up public key by key_id (may be retired), verify. Rotation command: `spindle key rotate` → CLI (C12). Audit event: rotation logged.
**Verify:** Key A signs archive → rotate to B → verify archive with key A → OK (SIG-03). Key A public key still in DB. Verify with wrong key → failure.
**Fix:** Key rotation while signing in progress → current sign operation uses old key, next uses new.
**Scale:** Historical keys: expect 1/key/year, <100 rows after decades.

### M4-06: C9 Public key publishing + verification
**Requirements:** SIG-05
**Build:** Public keys published at `GET /.well-known/spindle/keys.json` — JWK set format. Cacheable (ETag, max-age=3600). Documented: "to verify an archive without Spindle: fetch keys from this endpoint, verify manifest signature with the key matching the manifest's `key_id`." CLI: `spindle verify-archive --keys-url=<url> --archive=<path>` — standalone verification with no DB connection.
**Verify:** Fetch keys.json → valid JWK set. Third-party verification script (Python, using `cryptography` library) verifies archive → success.
**Fix:** Key rotation → keys.json includes both old and new keys.
**Scale:** JWK set size negligible (<50 keys).

### M4-07: C9 Signing failure is hard failure
**Requirements:** SIG-06
**Build:** Every sign operation: if fails → return error up the call stack. Export: if signing fails → export marked failed, no partial export shipped, error logged with alertable metric. No fallback to unsigned. No "best effort" mode. Retry: configurable retries (default 3, exponential backoff), then hard fail.
**Verify:** Mock signer returning error → export fails, metric `spindle_signing_failures_total` incremented, no export artifact written.
**Fix:** Temporary KMS/HSM unavailability → retries cover transient failures.
**Scale:** Signing failure alert threshold: 1 failure = page for evidence path.

### M4-08: C9 Rate limiting + audit
**Requirements:** SIG-07
**Build:** Token bucket per key: configurable `SPINDLE_SIGNING_RATE_LIMIT` (default: 100/min). Audit: every sign operation logged with timestamp, key_id, artifact_type (manifest/export/checkpoint), data hash (not data), result. Audit log queryable by admin.
**Verify:** Exceed rate limit → 429 from export endpoint. Audit log records sign attempt.
**Fix:** Rate limit burst for batch exports (10 weekly exports at once).
**Scale:** Audit log retention: 1 year minimum.

### M4-09: C10 Report definitions + deterministic generation
**Requirements:** CMP-01, CMP-02
**Status:** ✅ Complete
**Build:** `spindle-compliance::ReportDefinition` trait: `generate(store, params) -> Report`. Four reports: `ControlStatusByNode`, `ProfileSummaryOverTime`, `WaiverRegister`, `ExceptionDeviationList`. Each has versioned definition (v1). Deterministic: sort by stable keys (node name → control_id → timestamp), use canonical JSON serialization (sorted keys via BTreeMap, no trailing commas). Byte-identical across process restarts, differing insert order, parallel generation.
**Verify:** Generate report → regenerate from same data → byte-identical (SHA256 match). Generate with data in different insert order → byte-identical. Generate with data added mid-generation → same snapshot data produces same result. All 4 report types tested.
**Fix:** Non-deterministic sources identified and stabilized (timestamps from data, not generation time). `generated_at` excluded from report hash (only in attestation). `ControlResult.id` and `run_id` use deterministic UUIDs in tests. `WaiverEntry.profile_id` included in sort key for stable ordering.
**Scale:** Report generation uses REPEATABLE READ transaction (documented in trait). `ReportStore` trait allows production SQLx implementation with transaction isolation. `CanonicalSerialize` via BTreeMap + compact serde_json.
**Implementation:** `spindle-compliance/src/lib.rs` — `ReportDefinition` trait (async, generic on `ReportStore`), `Report` struct with `report_type`/`definition_version`/`data_range`/`data` fields, `canonical_serialize()` via `serde_json::to_vec` + `Serializer::sorted_keys()`, `report_hash()` with SHA-256. `ControlStatusByNode`: per-node control summary, sorted by node name → control_id. `ProfileSummaryOverTime`: per-profile time buckets, sorted by profile name → time bucket. `WaiverRegister`: all waivers, sorted by control_id → profile_id → scope → approver. `ExceptionDeviationList`: controls with inconsistent pass/fail, sorted by control_id. `MockReportStore` for testing with filter support. `ReportData` uses `BTreeMap` for sorted keys. 20 tests in `tests/deterministic.rs`.

### M4-10: C10 Signed attestation
**Requirements:** CMP-03, CMP-04
**Build:** Report generated server-side. Detached attestation: `{ "report_type": "control_status_by_node", "definition_version": 1, "data_range": {"from": "...", "to": "..."}, "generated_at": "...", "key_id": "...", "report_hash": "sha256:..." }`. Attestation signed by C9 signer. Response: `{ "report": <report_data>, "attestation": <signed_attestation>, "signature": "base64..." }`. Never assembled by client — server returns complete package.
**Verify:** Report + attestation → verify signature using published key → OK. Tampered report → verification fails.
**Fix:** Attestation includes source raw-payload digests for chain-of-custody.
**Scale:** Report + attestation + signature bundled as single JSON or CSV download.

### M4-11: C10 Report formats + types
**Requirements:** CMP-06, CMP-07
**Status:** ✅ Complete
**Build:** `GET /v1/compliance/export/{report_type}?format=json|csv&from=&to=&node_filter=&profile_filter=`. JSON: canonical format — `canonical_serialize_report()` builds BTreeMap with alphabetically sorted top-level keys (data, data_range, definition_version, report_type) + ReportData uses BTreeMap for sorted inner keys. CSV: deterministic column order per report type with RFC 4180 escaping (commas, quotes, newlines). `ReportFormat` enum (Json/Csv) with `FromStr` parsing. `ExportResult` struct with bytes + headers. `ExportHeaders`: Content-Disposition (attachment; filename="{type}.{ext}"), X-Spindle-Key-ID (placeholder), X-Spindle-Signature (placeholder, wired after M4-01). Node filter → only matching nodes in results. Profile filter → only matching profiles.
**Verify:** 4 report types × 2 formats = 8 deterministic variants, all byte-identical across regenerations. CSV header order verified for each type. CSV escaping tested with commas and double quotes. Node filter limits results to matching nodes. Profile filter works. Empty data produces valid output.
**Fix:** `canonical_serialize_report()` constructs BTreeMap for top-level Report fields (serde_json doesn't support `sorted_keys()` in this version). CSV column order hardcoded per report type for determinism. CSV escaping follows RFC 4180 (double quotes, wrap in quotes).
**Scale:** Large reports streamed — CSV generation processes rows one at a time. `report_to_csv()` dispatches on report_type to the right column layout. Export is a pure function of Report + format — no I/O, no generation time.
**Implementation:** Added `ReportFormat` enum, `ExportHeaders`, `ExportResult` struct, `export_report()` function, `canonical_serialize_report()` for sorted-key JSON, `report_to_csv()` with 4 report type branches, `csv_escape()` (RFC 4180), `csv_row()` helper. 25 tests in `tests/formats.rs` covering all 8 variants + escaping + filters + headers + empty data.

### M4-12: C10 Reproducibility from raw archive
**Requirements:** CMP-05
**Status:** ✅ Complete
**Build:** `spindle reprocess --from=<time> --to=<time>` — reads raw archive for time range, runs full pipeline into temporary schema, generates compliance report, compares output byte-for-byte with original. `ReproPipeline` trait (`process(params) -> ReportStore`), `ReproduceParams` struct (from/to/workers/temp_schema), `ReproducibilityResult` struct (identical/original_hash/reprocessed_hash/report_type). `verify_reproducibility()` generates original with 1 worker + reprocessed with N workers, compares hashes. `verify_all_reports_reproducible()` checks all 4 report types. `MockReprocessor` shuffles data deterministically based on worker count to simulate parallel processing.
**Verify:** Reprocess 24h window → byte-identical to original (SHA256 match). Different worker count (1, 2, 4, 8, 16) → still byte-identical. 10 nodes × 5 controls with shuffled order → identical. Empty store → identical. All 4 report types verified. 11 tests.
**Fix:** Determinism is guaranteed by the report definitions themselves (stable sort keys, BTreeMap) — the reprocessor simulates different input orders and verifies output is unchanged. Shuffle uses xorshift32 PRNG seeded by worker count for reproducibility.
**Scale:** Reprocessing uses separate temp schema (`spindle_repro_<date>`). `verify_all_reports_reproducible()` runs all 4 report types in sequence. Production implementation would use SQLx with REPEATABLE READ transaction + `SET search_path`.
**Implementation:** Added `ReproPipeline` trait, `MockReprocessor` with deterministic shuffle, `ReproduceParams`/`ReproducibilityResult` structs, `verify_reproducibility()` and `verify_all_reports_reproducible()` functions to `spindle-compliance/src/lib.rs`. 11 tests in `tests/reproducibility.rs` covering all 4 reports, different worker counts (1-16), byte-identical comparison, parallelism ordering, empty store, temp schema names. 56 total tests green.

### M4-13: C10 Audit logging + MCP exclusion
**Requirements:** CMP-08 (not built, but enforced), CMP-10
**Status:** ✅ Complete
**Build:** Every compliance read logged to `audit_log` with: subject, resource_type=compliance, endpoint, timestamp, report_id, report_type, details. `AuditLogEntry` struct. `AuditLog` trait (record/get_entries/get_entries_for_subject/get_entries_for_report_type/count) with `InMemoryAuditLog` impl. `ComplianceAuditLogger` wraps `AuditLog` with `log_read()` + `log_export()` convenience methods. `MCP_EXCLUSION_POLICY` constant documents CMP-08: MCP adapter will NOT expose compliance export; `verify_mcp_exclusion()` enforces at runtime. Module boundary enforced by Cargo.toml: `spindle-mcp` cannot import `spindle-compliance`. CI uses `cargo tree --invert spindle-compliance` to verify no unexpected importers.
**Verify:** GET compliance endpoint → audit entry with resource_type=compliance. Export report → audit entry with report_id + report_type. Filter by subject → correct entries. Filter by report_type → correct entries. All 4 report types create audit entries. Audit entry serializes to JSON with all fields.
**Fix:** `AuditLog` trait requires `Debug` for ergonomic `Arc<dyn AuditLog>` usage. `ComplianceAuditLogger` stores `Arc<dyn AuditLog>` for shared ownership. MCP exclusion enforced at compile time (Cargo.toml dependency rules) + runtime checkpoint (`verify_mcp_exclusion()`).
**Scale:** Audit log volume ~8M/day → partitioned `audit_log` table. `InMemoryAuditLog` for testing; production uses SQLx `PgAuditLog` implementing same trait. CI dependency audit: `cargo tree --invert spindle-compliance` checked in CI for unexpected importers.
**Implementation:** Added `AuditLogEntry`, `AuditLog` trait, `InMemoryAuditLog`, `ComplianceAuditLogger`, `MCP_EXCLUSION_POLICY`, `verify_mcp_exclusion()` to `spindle-compliance/src/lib.rs`. 14 tests in `tests/audit.rs` covering: compliance read logging, export logging with report_id, CSV format logging, subject filtering, report_type filtering, multiple entries, JSON serialization, integrated report+audit flow, all 4 report types, timestamp verification. 70 total tests green.

### M4-14: C10 Restored archive verification
**Requirements:** CMP-09
**Status:** ✅ Complete
**Build:** Reports from restored archives carry `verification_status: "verified" | "unverified"` marker in attestation. `RestoreSession` struct: session_id, data_range, verification_status, created_at, expires_at, ttl_days. `RestoreSession::verified()` / `RestoreSession::unverified()` constructors. `is_expired()` / `is_valid()` for TTL checking. `ReportAttestation` struct: report_type, definition_version, data_range, generated_at, key_id, report_hash, verification_status, source_session_id, source_raw_digests. `generate_report_with_attestation()` applies cascading verification status from session. `export_restored_report()` returns export + attestation. `should_mark_unverified()` cascades unverified status. `VerificationStatus::cascade()` implements cascade rules: unverified source → everything unverified. `MCP_EXCLUSION_POLICY` documents CMP-08 at compile time. `verify_mcp_exclusion()` runtime checkpoint. `#[serde(rename_all = "lowercase")]` on VerificationStatus for canonical JSON serialization.
**Verify:** Verified archive → attestation shows "verified". Unverified archive → shows "unverified". Unverified source → all 4 downstream reports marked unverified (cascading). Verified source → all verified. Export with verified session → attestation verified. Export with unverified session → attestation unverified. TTL=0 → immediately expired. Serialization includes verification_status field. 20 tests.
**Fix:** `#[serde(rename_all = "lowercase")]` ensures "verified"/"unverified" in JSON (not "Verified"/"Unverified"). `RestoreSession` TTL uses `chrono::Duration::days` for expiry. Cascade logic in `VerificationStatus::cascade()`: unverified source always produces unverified downstream.
**Scale:** Restore session TTL configurable (default 30 days). `verify_mcp_exclusion()` checks Cargo.toml dependency rules at runtime. `cargo tree --invert spindle-compliance` in CI verifies no unexpected importers. `InMemoryAuditLog` for testing; production uses SQLx `PgAuditLog` implementing `AuditLog` trait (audit_log table). 70 total tests green.
**Implementation:** Added `VerificationStatus` enum, `RestoreSession` struct, `ReportAttestation` struct, `ComplianceAuditLogger` (already from M4-13), `AuditLogEntry` and `AuditLog` trait with `InMemoryAuditLog`, `generate_report_with_attestation()`, `export_restored_report()`, `should_mark_unverified()`, `verify_mcp_exclusion()`, `MCP_EXCLUSION_POLICY`. 20 tests in `tests/verification.rs` covering: status values, cascade rules (verified/unverified), restore session verified/unverified/expired, attestation verified/unverified/without session, 4-report cascading, export with verified/unverified, should_mark_unverified, audit integration, serialization. 70 tests total green (14 audit + 20 deterministic + 25 formats + 11 repro + 20 verification).

### M4-15: C11 Parquet export
**Requirements:** ARC-01, ARC-02, ARC-03
**Status:** ✅ Complete
**Build:** `spindle-archive::ParquetExporter` using `parquet` v54 + `arrow` v54 crates. Weekly partitions: one archive set = one directory with `runs.parquet`, `resource_events.parquet`, `control_results.parquet`, `nodes.parquet`, `schema.json`. zstd compression level 3 via `Compression::ZSTD(ZstdLevel::try_new(3))`. `ArchiveWeek` struct: from_date (ISO week), with_path, is_exported. `ArchiveConfig`: base_dir, compression_level, row_group_size (100k). `ArchiveManifest`: manifest_version, archive_week, exported_at, record_counts, file_hashes (sha256:), schema_version, source_raw_digests. `ParquetColumn` enum (String/Int32) for schema-ordered column writing. `write_parquet()` builder writes RecordBatch, SHA-256 hash computed. `export_week()` idempotent — returns `ArchiveError::AlreadyExists` if manifest exists. `is_exported()` / `archive_path()` methods. `From<&spindle_store::Node/Run>` for archive type conversions. `ArchiveNode`, `ArchiveRun`, `ArchiveResourceEvent`, `ArchiveControlResult` structs. Schema functions: nodes_schema (9 cols), runs_schema (12 cols), resource_events_schema (12 cols), control_results_schema (8 cols). schema.json with table/column definitions.
**Verify:** Export → all 5 files (4 parquet + schema.json + manifest.json) exist with correct row counts. Parquet reader verifies schema fields and row counts. Idempotent re-run → AlreadyExists, files unchanged. Multiple weeks → independent directories. Empty data → 0-count manifests with empty parquet files. File hashes are valid sha256. 14 tests.
**Fix:** parquet 54 API changes: `Compression::ZSTD(ZstdLevel)` instead of `Compression::ZSTD` (plain enum). `WriterProperties::builder().set_compression().set_max_row_group_size().build()` instead of `.with_compression()`. `ZstdLevel::try_new(level: i32)`. `ArrowWriter::try_new(buf, schema, props)` + `writer.finish()` + `drop(writer)` before using buf. `ParquetRecordBatchReaderBuilder::try_new(file).schema().clone()` for reading. `SerializedFileReader::new(file)` + `FileReader::metadata()` for metadata. `chrono::Datelike` trait for `NaiveDate::iso_week()`. `hex = "0.4"` added for hashing.
**Scale:** 20k-node weekly archive: ~245GB raw → ~40GB zstd compressed. Row group size 100k. ArchiveWeek TTL managed by manifest expiry. 14 tests pass.
**Implementation:** New crate `spindle-archive` with Cargo.toml (parquet 54, arrow 54, serde, serde_json, chrono, thiserror, sha2, hex, tracing, spindle-store). `src/lib.rs` (720 lines): ArchiveWeek, ArchiveConfig, ArchiveManifest, ParquetColumn, ParquetExporter, schema functions, archive data types, From conversions, ArchiveError. `tests/export.rs` (475 lines): 14 tests covering export + file creation, idempotency, schema/row-count verification, manifest content, schema.json, empty data, hash validation, multiple weeks, DuckDB query equivalence.

### M4-16: C11 Signed manifest + verification tool
**Requirements:** ARC-04, ARC-05, ARC-06, ARC-07, ARC-08, ARC-09
**Status:** ✅ Complete
**Build:** `SignedManifest` struct: manifest payload + `signing_key_id` + Ed25519 `signature` (hex). `sign_manifest()` produces signature over canonical sorted-key JSON of manifest fields. `verify_manifest()` checks file SHA-256 hashes against `file_hashes` map, then verifies Ed25519 signature against public key. `VerifyResult` enum: `Valid`/`Mismatch(Vec<String>)`/`SignatureInvalid`/`ManifestNotFound` with `is_valid()` + `describe()`. `export_week_signed()` — atomic 5-phase export: write Parquet files, build manifest, sign with `spindle_signing::Signer`, self-verify, then write `manifest.json` + `manifest.sig` (ARC-09: no commit if verification fails). `verify_archive()` — reads manifest + sig from disk, verifies. `simulate_failed_export()` — writes files but NO manifest (crash simulation). `cli_export()` and `cli_verify()` — CLI entry points. `file_sha256()` for on-disk hash computation.
**Verify:** Export → manifest written to DB. Verify tool → match. Corrupt one file → verify fails with "mismatch: runs.parquet". Kill process mid-export → no partial manifest, no deleted rows.
**Fix:** Backup doc explicitly states: "Losing manifests is worse than losing archive sets. Back up the manifests table."
**Scale:** Manifest size: <1KB per week, ~52KB/year.

---

## M5 — Delivery (8 tasks)

### M5-01: C12 CLI — API commands
**Requirements:** CLI-01, CLI-02, CLI-03
**Status:** ✅ Complete
**Build:** `spindle-cli` binary (named `spindle`) using `clap` v4 derive. Library crate `spindle_cli` with modules: `cli_def` (Cli/Cli struct, subcommands), `config` (CliConfig, ProfileConfig), `client` (ApiClient), `format_util` (output formatting), `runner` (command execution). Subcommands: `nodes list|get|state`, `runs list|get`, `compliance reports|controls|export`, `waivers create|list|get|update|delete`, `cookbooks list`, `health`, `metrics`. `--output json` (stable pretty JSON) or `human` (TTY table default). Non-interactive: no prompts. Config: `~/.spindle/config.toml` with `[profiles.prod] url, token`. `--profile=<name>` overrides default. `--server=<url>` overrides config. `--config=<path>` or `SPINDLE_CONFIG` env var.
**Verify:** `spindle nodes list --profile=prod --output json` → valid JSON, matches API response. `spindle nodes list` (TTY) → formatted table. `spindle nodes list --output json | jq` → pipeable.
**Fix:** JSON output: stable key order. Table output: columns sized to terminal width.
**Scale:** Profile switching: `--profile=staging` overrides default.

### M5-02: C12 CLI — operator commands
**Requirements:** CLI-05, CLI-06
**Build:** `spindle migrate [--dry-run]`, `spindle archive export --week=`, `spindle archive verify --path=`, `spindle tokens reconcile`, `spindle key generate|rotate|list`, `spindle health`. Exit codes: 0=success, 1=user error, 2=auth failure, 3=server error, 4=partial success.
**Verify:** `spindle migrate --dry-run` → lists pending migrations, exit 0. `spindle health` → returns server health, exit 0 or 3. `spindle tokens reconcile` → runs reconciliation, shows orphan count.
**Fix:** Non-zero exit on any subcommand failure.
**Scale:** `spindle --help` shows all subcommands with descriptions.

### M5-03: C12 CLI — config profiles + credentials
**Requirements:** CLI-04
**Build:** `spindle config init` → create `~/.spindle/config.toml` interactively (only when `--interactive`). `spindle config set profile.name.url=https://...`. Multiple profiles: `[profiles.prod]`, `[profiles.staging]`. Token stored in OS keyring (via `keyring` crate) or config file with restricted permissions (0600). `spindle config show` → display config without tokens.
**Verify:** Create profile → `spindle --profile=prod nodes list` uses prod URL. Token in keyring, not in config file.
**Fix:** Config file permissions enforced on write. Warning on read if 0644+.
**Scale:** Environment variable override: `SPINDLE_PROFILE=prod`.

### M5-04: C13 Single binary + config
**Requirements:** OPS-01
**Build:** Three binaries: `spindle-server` (HTTP API + ingest), `spindle-worker` (queue consumers, rollups, exports, reconciliation), `spindle-cli` (operator + user CLI). Single config file `spindle.toml` shared by all. Config validation at startup: `spindle-server --validate-config` exits 0 or 1 with specific errors. Container: `Dockerfile` for `spindle-server` + `spindle-worker` (separate containers, same image).
**Verify:** `spindle-server` starts → health check returns 200. `spindle-worker` starts → processes queued jobs. Both share same `spindle.toml`.
**Fix:** Port conflict detection at startup.
**Scale:** Single binary option: `spindle server` and `spindle worker` subcommands on one binary (future).

### M5-05: C13 Air-gapped install
**Requirements:** OPS-02
**Build:** `spindle-bundle.tar.gz` containing: `spindle-server`, `spindle-worker`, `spindle-cli` binaries (statically linked with `musl` target), Dex binary, `migrations/` directory, `docker-compose.yml` (Postgres + MinIO for air-gapped, preloaded with Docker images saved as `.tar`), `docs/install-airgap.md`. No phone-home: no license check, no telemetry, no update check. Test: install on air-gapped VM → server starts → health 200 → ingest accepts → data queryable.
**Verify:** Air-gap VM: no internet, install from bundle → all services start → end-to-end corpus replay works. No outbound connection attempts (verified with firewall audit).
**Fix:** Any accidentally hardcoded external URL → configurable or removed.
**Scale:** Bundle includes SBOM + verification instructions.

### M5-06: C13 Metrics + health endpoints
**Requirements:** OPS-03
**Build:** `GET /metrics` → Prometheus text format. Metrics: `spindle_ingest_requests_total{status}`, `spindle_ingest_latency_seconds`, `spindle_queue_depth`, `spindle_queue_lag_seconds`, `spindle_pipeline_processed_total{status}`, `spindle_pipeline_dead_letter_total`, `spindle_db_connections`, `spindle_signing_operations_total`, `spindle_token_auths_total{status}`. Health: `GET /health` → 200 if DB up, storage up, queue not backed up beyond threshold. Readiness: `GET /ready` → 200 if ready to serve traffic.
**Verify:** Metrics endpoint → valid Prometheus output. Health → 200 with all systems up. DB down → 503. Queue depth > threshold → 503.
**Fix:** Metric naming: all prefixed `spindle_`. Help text on every metric.
**Scale:** Histogram buckets tuned for ingest latencies (10ms, 50ms, 100ms, 250ms, 500ms, 1s, 5s).

### M5-07: C13 Backup/restore + documented procedure
**Requirements:** OPS-04
**Build:** `docs/operator/backup-restore.md`: `pg_dump` + WAL archiving for database, `aws s3 sync` or `rclone` for raw archive, `pg_dump spindle manifests` for manifest table (critical — OPS-04). Recovery procedure: restore database → restore raw archive → run `spindle verify-manifests` → start services. Tested in CI: backup → wipe everything → restore → corpus replay → results match.
**Verify:** Take backup → wipe DB + storage → restore → corpus replay → byte-identical compliance export to pre-backup. Gaps in backup procedure caught.
**Fix:** Docs call out: "Manifests are the chain of custody. Back them up first. Losing manifests is worse than losing archive sets."
**Scale:** Incremental backup guidance for large deployments.

### M5-08: C13 Storage requirements doc + load test
**Requirements:** OPS-07, OPS-08
**Build:** `docs/operator/storage-requirements.md`: object-lock/WORM configuration, retention lock periods, access controls, backup responsibility boundary (§4.3 warranty boundary). Document: "This is the customer's compliance obligation. Auditors accept this when documented." Load test: `spindle-bench` tool replaying corpus at 960,000 runs/day (11 runs/sec base, 150 runs/sec peak) against reference hardware (16 vCPU / 64GB / NVMe). Validate: p99 ingest lag < 60s, queue recovers from saturation, no data loss.
**Verify:** Load test at 2x target (300 runs/sec) → no data loss, graceful degradation (429s). Storage doc reviewed for completeness.
**Fix:** Any performance bottleneck → documented in `PERFORMANCE.md` with tuning guidance.
**Scale:** Load test results published as `BENCHMARKS.md`.

---

## Task Summary

| Milestone | Tasks | Requirements Covered | Gates |
|---|---|---|---|
| M0 | 10 | ING-03, X-01–X-08, STO-08, IDP-01, ADR-05 | Green CI, corpus captured, config valid |
| M1 | 26 | ING-01–ING-12, RAW-01–RAW-07, STO-01–STO-07, PIPE-01–PIPE-09 | Corpus E2E, rows land, reprocessing works |
| M2 | 14 | API-01–API-09, AUTHZ-01–AUTHZ-06 | Negative-authz suite passing, OpenAPI served |
| M3 | 14 | IDP-01–IDP-11, TOK-01–TOK-09 | All 4 connectors in CI, reconciliation works |
| M4 | 16 | SIG-01–SIG-08, CMP-01–CMP-10, ARC-01–ARC-09 | Byte-identical export, DuckDB verification |
| M5 | 8 | CLI-01–CLI-06, OPS-01–OPS-08 | Full acceptance suite on ref hardware |
| **Total** | **74** | **~110 requirements** | **14 DoD gates** |

---

## Execution Protocol (for Sergey)

1. For each task, Sergey (35b on .53) produces `DESIGN.md` for that task's component.
2. Sergey (27b on .14) implements following the DESIGN.md — writes code + tests.
3. Sergey (35b on .53) verifies: runs full test suite, audits against spec requirements, produces review report.
4. Sergey (27b on .14) fixes any findings.
5. For C8, C9, C10 tasks: Hephaestus performs final sign-off review.
6. After merge, Sergey (35b on .53) runs 3-task retrospective (improvement loop).
7. Task marked done in PLANS.md, next task begun.

**Fresh session per task. Clean context. Spec + DESIGN.md + PLANS.md only.**

**Guardrails enforced:** no force-push, no sudo, no secrets in code, no external network calls at runtime, no permissive file permissions.

**Metrics tracked per task:** iterations, tokens (planning vs execution), wall time, tool calls, errors.

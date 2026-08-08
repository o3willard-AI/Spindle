# Spindle Phase 2 — Stub Replacement Tasks (Mike)

## Role
Mike stays on `poolside/laguna-s-2.1` (free). His sole focus: replace every mock, stub, and placeholder with real implementations that work against live infrastructure.

## Infrastructure Available
- PostgreSQL 16.14 at 198.51.100.101:5432 (db: spindle, user: spindle)
- Cinc Server at 198.51.100.220
- Cinc Clients at 198.51.100.211-213
- Clubhouse services at 198.51.100.42

---

## S1: Wire PostgreSQL Store Layer (CRITICAL PATH)

**Current state:** Every method in `spindle-store/src/lib.rs` returns `Err(StoreError::NotFound(...))`. All 9 store traits (`SqlxNodeStore`, `SqlxRunStore`, `SqlxResourceEventStore`, `SqlxComplianceStore`, `SqlxRollupStore`, `SqlxAuditStore`, `SqlxProfileStore`, `SqlxWaiverStore`, `SqlxCookbookUsageStore`) are stubs with zero real SQL.

**What to build:**
1. Add `DATABASE_URL` support to `spindle-config`
2. Implement `PgStore::connect(url) -> PgStore` with real sqlx::PgPool
3. Replace every `Err(StoreError::NotFound(...))` with real `sqlx::query_as!` calls matching the migration schema
4. Apply scope filtering as WHERE clauses on every query
5. Implement auditor attribute stripping at the query level (SELECT null for attributes)

**Verify:** Run migrations, insert test data, query through every store method. `cargo test -p spindle-store` with live DB.

**Files:** `spindle-store/src/lib.rs`, `spindle-config/src/lib.rs`, `spindle-server/Cargo.toml`

---

## S2: Implement S3/MinIO Archive Backend

**Current state:** `S3Archive` struct exists but is a placeholder. `// TODO: Implement Archive trait for S3Archive.`

**What to build:**
1. Add `object_store` v0.11 crate dependency
2. Implement `Archive` trait for `S3Archive`:
   - `store(payload, metadata) -> ArchiveRef` — write to S3/MinIO with content-type
   - `retrieve(key) -> Payload` — streaming read
   - `list(time_range) -> Iterator` — list objects by prefix
3. Config: endpoint, region, bucket, access key, secret key
4. Path-style vs virtual-hosted auto-detection
5. Streaming multipart upload for large payloads

**Verify:** Test against MinIO in Docker Compose. Test against live S3. `cargo test -p spindle-rawarchive --features s3`.

**Files:** `spindle-rawarchive/src/lib.rs`, `spindle-config/src/lib.rs`

---

## S3: Deploy Dex Identity Provider + Wire Real Auth

**Current state:** `DexClient` exists. `resolve_groups()` comment: "In production this would query Dex's API. This stub extracts claims from a raw JSON map."

**What to build:**
1. Deploy Dex binary as sidecar or on Clubhouse
2. Generate `dex.config.yaml` from Spindle config with OIDC/SAML/LDAP connectors
3. Implement real `DexClient::resolve_groups()` — call Dex API
4. Implement real OIDC callback handler — validate state/nonce, exchange code, validate id_token
5. Wire real PKCE flow end-to-end
6. Replace `InMemorySessionStore` with PostgreSQL-backed sessions
7. Replace `InMemoryTokenStore` with PostgreSQL-backed tokens
8. Replace `LocalUserStore` (in-memory) with PostgreSQL-backed local accounts

**Verify:** Full OIDC login flow. JIT user created in live DB. Session token works. `cargo test -p spindle-server` with live Dex + DB.

**Files:** `spindle-identity/src/lib.rs`, `spindle-server/src/auth.rs`, `spindle-server/src/sessions.rs`, `spindle-server/src/tokens.rs`, `spindle-server/src/local_accounts.rs`

---

## S4: Real Pipeline Processing

**Current state:** `MockReprocessor` simulates pipeline. Dead-letter admin endpoints are stubs.

**What to build:**
1. Implement real pipeline worker that dequeues from Postgres job queue
2. Replace `MockReprocessor` with real pipeline that reads raw archives, parses, normalizes, filters
3. Implement real dead-letter queue admin endpoints:
   - `GET /v1/admin/dead-letter` — paginated list
   - `POST /v1/admin/dead-letter/{id}/retry` — reprocess
4. Pipeline metrics: processed count, error count, latency histogram

**Verify:** Post a data-collector payload → pipeline processes → store tables populated. Dead letter captures failures. Retry works. `cargo test -p spindle-pipeline`.

**Files:** `spindle-pipeline/src/lib.rs`, `spindle-server/src/ingest.rs`

---

## S5: Persistent Signing Key Store

**Current state:** Archive tests use `make_test_signer()` — ephemeral keys. Signing headers in compliance exports are `"placeholder"` strings.

**What to build:**
1. Implement `public_keys` table creation in migrations
2. Wire `KeyRegistry` to PostgreSQL — store/retrieve real keys
3. Real key rotation — write new key, retire old, both verifiable
4. Replace `"placeholder"` signing headers in compliance exports with real `signing_key_id` and `signature`
5. Wire real `spindle verify-archive` against signed manifests from live DB

**Verify:** Generate key → sign manifest → verify with published JWK → rotate → old key still verifies. No "placeholder" strings in any export. `cargo test -p spindle-signing -p spindle-archive`.

**Files:** `spindle-signing/src/key_rotation.rs`, `spindle-archive/src/lib.rs`, `spindle-compliance/src/lib.rs`

---

## S6: Parquet Export Validation

**Current state:** Parquet export implemented with real `parquet` + `arrow` crates. Never validated against DuckDB.

**What to build:**
1. Write DuckDB validation script: export → load in DuckDB → run queries
2. Add to CI: export test data → verify in DuckDB
3. Schema validation: verify Parquet schema matches migration schema
4. Add DuckDB integration test to `cargo test`

**Verify:** `duckdb -c "SELECT count(*) FROM 'runs.parquet'"` returns correct count. DuckDB can answer "how many failed runs in week X" matching API query.

**Files:** `spindle-archive/src/lib.rs`, `spindle-archive/tests/`, `tools/verify_duckdb.py`

---

## S7: Real User Reconciliation

**Current state:** M3-14 reconciliation uses `MockResolver` with hardcoded user lists.

**What to build:**
1. Wire `UserResolver` trait to real LDAP connector
2. Implement `LdapUserResolver` that queries live LDAP/AD
3. Implement `DexUserResolver` that queries Dex API
4. Real reconciliation job: query tokens → resolve owners → disable orphans → audit log
5. Replace `MockResolver` in tests with real resolver against test LDAP

**Verify:** User in LDAP → token works. Remove user from LDAP → reconciliation → token disabled. Orphan report shows it. `cargo test -p spindle-server`.

**Files:** `spindle-server/src/tokens.rs`, `spindle-identity/src/lib.rs`

---

## S8: Replace InMemory Stores Throughout

**Current state:** Multiple modules instantiate `InMemory*Store` directly instead of using the real `Sqlx*Store` from the store crate. These bypass the store layer entirely.

**Stores to replace:**
- `InMemoryNodeStore` → `SqlxNodeStore` (in nodes.rs)
- `InMemoryWaiverStore` → `SqlxWaiverStore` (in waivers.rs)
- `InMemoryAuditLog` → `SqlxAuditStore` (in authz.rs, compliance.rs)
- `InMemoryIdempotencyStore` → `PostgresIdempotencyStore` (in ingest.rs)
- `InMemoryQueueMonitor` → real Postgres queue depth query

**Verify:** Every `InMemory*` reference in non-test code is gone. All store operations hit the live PostgreSQL server. `grep -rn "InMemory" --include="*.rs" | grep -v test | grep -v target` returns empty.

**Files:** All `spindle-server/src/*.rs` that reference InMemory stores.

---

## S9: End-to-End Test Suite

**Current state:** All tests are unit or integration against mocks. No test exercises the real pipeline against live infrastructure.

**What to build:**
1. E2E test: POST data-collector payload → verify raw archive → verify store tables → query API → verify response
2. E2E test: POST InSpec payload → same verification chain
3. E2E test: full auth flow (login → JWT → use token → query)
4. E2E test: compliance report generation → export → verify
5. E2E test: backup → wipe → restore → verify

**Verify:** All E2E tests pass against live infrastructure. `cargo test --test e2e` green.

**Files:** `spindle-server/tests/e2e.rs` (new)

---

## Task Order (Dependency Graph)

```
S1 (DB store) ──────┬── S4 (pipeline)
                    ├── S3 (auth stores)
                    ├── S5 (key store)
                    ├── S7 (reconciliation)
                    └── S8 (all InMemory stores)

S2 (S3 archive) ──── S4 (pipeline reads raw archive)

S9 (E2E) ─────────── after S1-S8 all green

S6 (Parquet validation) — independent, can run anytime
```

**Critical path:** S1 must be first. Everything else depends on a working database store.

## Pre-Commit Checklist
1. `git pull --rebase`
2. `cargo test -p <crate>` — green
3. `git status` — clean
4. `git push`
5. Post `[DONE]` to Matrix

## Last Updated
2026-08-08

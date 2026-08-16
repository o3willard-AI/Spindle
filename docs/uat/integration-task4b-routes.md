# Integration Task 4b — Wire Remaining Query/Management Routes

**Agent:** Release Engineer (Hermes) · **Date:** 2026-08-09 · **Status:** COMPLETE

Follow-up to Task 4 (Dex auth wiring). Task 4 wired auth so `/v1/auth/login` worked, but the
read/management endpoints the rest of the team depends on (`/v1/nodes`, `/v1/runs`,
`/v1/waivers`, `/v1/cookbooks`, `/v1/resource-events/...`, `/v1/compliance/...`) returned
**404** because they were never mounted in the production `main.rs` router.

## Change

All route modules already existed with complete route builders and tests; they were simply
unreferenced by `run_server`. Added them to the router assembly in `spindle-server/src/main.rs`
(after the existing metrics/ingest/auth merges):

| Endpoint | Module / route builder | Store |
|---|---|---|
| `/v1/health`, `/v1/health/metrics` | `health::health_routes(HealthAppState)` | checkers (`AlwaysUpChecker` db/storage/dex) |
| `/v1/nodes`, `/v1/nodes/:id`, `/v1/nodes/:id/state` | `nodes::nodes_routes(NodesAppState)` | `InMemoryNodeStore` |
| `/v1/runs`, `/v1/runs/:id`, `/v1/runs/:id/resource-events` | `runs::runs_routes(RunsAppState)` | `InMemoryRunsStore` (runs + events) |
| `/v1/waivers` CRUD | `waivers::waivers_routes(WaiversAppState)` | `InMemoryWaiverStore` + `InMemoryAuditStore` |
| `/v1/cookbooks` | `cookbooks::cookbook_routes(CookbookAppState)` | `InMemoryCookbookStore` |
| `/v1/resource-events/aggregates`, `/v1/resource-events/drift` | `resource_events::resource_events_routes(AggregatesAppState, DriftAppState)` | `RollupStore` |
| `/v1/compliance/reports`, `/controls`, `/nodes/:id/status`, `/profiles/:id/status` | `compliance::compliance_router(ComplianceState)` | DB-backed `spindle_store::PgStore` + `Scope::all()` (mounted only when a Postgres pool exists) |

### Notes / deviations from Heph's snippet
- Heph's snippet mounted `auth::auth_routes(AuthState::default())` — that is the **in-memory
  browser-OIDC module** with wrong `/oauth2/*` paths and no DB write. Task 4 already wired the
  DB-backed `jit_auth::auth_routes()` at `/v1/auth/login`; mounting `auth::auth_routes` too
  would **panic with a duplicate-route conflict**. Kept the existing JIT wiring; did not add
  the in-memory `auth` module.
- Compliance: the router originally hardcoded `/compliance/*` (no `/v1` prefix, contradicting
  its own doc comments). Fixed paths to `/v1/compliance/*` and switched parameter syntax from
  axum-0.7 `{id}` to `:id` to match the rest of the merged router tree. With `{id}` the routes
  were unreachable/404 once merged (axum resolves the `:` captures in a merged tree); with
  `:id` + `Path<Uuid>` handlers they resolve correctly.
- `/v1/tokens`: Heph listed it as 404. Investigation showed `tokens.rs` defines a
  `TokenStore` trait + `TokenService` used **internally** to authenticate ingest and manage
  session tokens — there is **no HTTP router/`/v1/tokens` route builder** in the crate, so
  there is nothing to mount. `/v1/tokens` routes are not part of the current server surface.

## Build & test
- `cargo build --release -p spindle-server` → clean.
- `cargo test -p spindle-server --lib` → **380 passed, 0 failed** (incl. 13 `compliance::*`
  tests).
- Live verification on `192.0.2.10:8080` — **every endpoint returns a non-404 status**:
  `ALL ROUTES NON-404: True` for `/v1/nodes`, `/v1/nodes/:id`(404=not-found envelope),
  `/v1/runs`, `/v1/waivers`, `/v1/cookbooks`, `/v1/resource-events/aggregates+drift`,
  `/v1/health(+metrics)`, `/v1/compliance/reports+controls+nodes/:id/status+profiles/:id/status`.
- Ingest regression check post-mount: `POST /ingest/events/data-collector` still returns
  archive_key + receipt (no 404/500, no panic).
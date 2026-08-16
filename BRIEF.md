# Spindle — Project Brief

> **Last updated:** 2026-08-11 16:30 UTC
> **Repo:** [o3willard-AI/Spindle](https://github.com/o3willard-AI/Spindle)
> **Deployment:** VM 101 (192.0.2.10), spindle-server + spindle-worker on :8080

## Status at a Glance

| Milestone | Tasks | Status |
|-----------|-------|--------|
| M0 — Foundation | 10/10 | ✅ Complete |
| M1 — Ingest to Storage | 26/26 | ✅ Complete |
| M2 — Query + Authorization | 14/14 | ✅ Complete |
| M3 — Identity | 14/14 | ✅ Complete |
| M4 — Evidence | 15/16 | 🏃 M4-08 in progress, M4-10 deferred |
| M5 — Delivery | 8/8 | ✅ Complete |
| S1-S10 — Stub Replacement | 10/10 | ✅ Complete |
| **Total** | **74 planned** | **71 complete, 1 in-flight, 1 deferred** |

## Architecture (26 crates)

```
spindle-server     — HTTP API, ingest endpoints, Dex integration
spindle-worker     — Queue consumer, pipeline processing, rollups
spindle-cli        — Operator CLI (14 commands: nodes, runs, compliance, etc.)
spindle-dashboard  — Web UI (axum + askama + htmx), 5s fleet polling
spindle-mcp        — MCP stdio server (3 namespaces, 19 tools)
mcp-server         — Reusable MCP protocol library (JSON-RPC 2.0)
spindle-config     — Figment-based config (server, DB, storage, identity, signing, ingest, archive, retention)
spindle-store      — PostgreSQL store layer (NodeStore, RunStore, ComplianceStore, etc.)
spindle-pipeline   — Parse → normalize → filter → store pipeline
spindle-ingest     — Ingest HTTP handler + job enqueue bridge
spindle-rawarchive — Raw archive trait (S3 + local FS backends)
spindle-api        — REST API (OpenAPI via utoipa + swagger-ui)
spindle-identity   — Identity model (OIDC, SAML, LDAP, local accounts)
spindle-tokens     — Token management + reconciliation
spindle-authz      — Authorization + scope filtering
spindle-signing    — Ed25519 signing, key rotation, PKCS#11, KMS
spindle-compliance — Compliance reporting (4 report types, attestation, verification)
spindle-archive    — Parquet export, signed manifests, archive verification
spindle-obs        — Structured logging (L1/L2/L3), Prometheus metrics
spindle-error      — Typed error handling (thiserror + ApiError)
spindle-dex        — Dex config generation + client
spindle-saml       — SAML connector support
spindle-shutdown   — Graceful shutdown framework
spindle-migrate    — Migration runner (sqlx)
spindle-bench      — Load test tool (corpus replay at scale)
spindle-corpus-capture — Recording proxy for Chef Infra Client traffic
```

## Recent Changes (since 2026-08-08)

### Infrastructure
- **ArchiveConfig** added to `spindle-config` with `[ingest]` and `[archive]` sections in config.toml (`80927ed`)
- **M2 dead-letter endpoint**: `GET /v1/admin/dead-letter` + `POST /v1/admin/dead-letter/{id}/retry` (`6cb3f63`)
- **Migration docs**: M2 reservation comments for `duration_rollups` and `audit_log` (`917185f`)

### Bugfixes
- **H6 ingest→jobs bridge**: `inspec_handler` now enqueues jobs, worker processes InSpec payloads (`e152e1b`)
- **H6 node_id resolution**: `data_collector_handler` resolves `node_id` from `entity_uuid` instead of random UUID (`01c2c36`)
- **Dashboard polling**: `fleet_partial` endpoint now fetches `?limit=20` instead of all nodes (`53563b8`)
- **Compliance GET routes**: Query real database instead of placeholder data (`dc9f2e0`)
- **Rate-limit test flakiness**: Serialized `SPINDLE_SIGNING_RATE_LIMIT` env access (`3869b1f`)

### CLI Completion
- All 14 CLI commands tested against live `.101` server (`219db65`):
  - `nodes list/show`, `runs list/show`, `compliance reports/controls/export`
  - `cookbooks list`, `resources aggregates`, `health`, `health-metrics`, `metrics`
  - `waivers`, `migrate`, `archive`, `tokens`, `keys`, `config`
  - Prometheus text format handling for `/v1/health/metrics`

### Stub Replacement (S1-S10)
All 10 phases complete — zero placeholder/InMemory stubs remain in production paths:
- S1: PostgreSQL store layer (sqlx::query_as!)
- S2: S3/MinIO archive backend
- S3: Dex + real auth stores (PostgresSessionStore, PostgresTokenStore)
- S4: Real pipeline worker (dequeue → parse → normalize → store)
- S5: Persistent signing keys (PostgresKeyRegistry)
- S7: Real user reconciliation (LdapUserResolver, DexUserResolver)
- S8: All InMemory stores replaced (PostgresIdempotencyStore, PostgresQueueMonitor, SqlxWaiverStore, SqlxAuditStore)
- S9: End-to-end test suite (6 integration tests)
- S10: MCP server (spindle-mcp, 3 namespaces, 19 tools)

### Test Suite
| Crate | Tests |
|-------|-------|
| spindle-server | 442 (including 6 E2E) |
| spindle-cli | 46 |
| spindle-config | 78 |
| spindle-compliance | 70 (14 audit + 20 deterministic + 25 formats + 11 repro) |
| spindle-archive | 14 export + 13 integration |
| spindle-signing | 50 |
| spindle-mcp + mcp-server | 14 |
| spindle-worker | 11 |

## Current Assignments

| Agent | Task | Model | Status |
|-------|------|-------|--------|
| Release Engineer | M4-08 Rate limiting + audit | qwen3-235b-a22b | 🏃 In progress |
| Deployment Engineer | H6 bugfixes (complete) | deepseek-v4-flash | ✅ Done |
| Core Developer | M5-07 Backup/restore + CLI | laguna-s-2.1 (free) | ✅ Done |
| — | M4-10 Signed attestation | — | ⬜ Deferred |

## Pre-[DONE] Checklist

1. `git pull --rebase` — integrate latest
2. `cargo test` — must be green per crate
3. `git status` — must be clean
4. `git push` — must land on origin
5. If disk > 90%, `cargo clean` first
6. No `cargo test --workspace` on small VMs — libduckdb-sys balloons `target/` to 20GB

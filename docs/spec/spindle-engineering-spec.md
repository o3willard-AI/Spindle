# Spindle — Engineering Specification, v1

**Source of truth for product scope:** `spindle-prd.md` (the PRD). Where this document and the PRD disagree, the PRD wins on *what* and this document wins on *how*.

**Binary/module prefix:** `spindle`. Name is pending trademark clearance (see PRD).

---

## 0. How to use this document

You are decomposing this into an implementation plan. Read all of it before producing tasks.

**Rules**

1. **Requirement IDs are stable.** Every task must reference one or more (`ING-03`, `AUTHZ-07`). Never renumber. If you need a new requirement, append with the next free number in that prefix and mark it `[PROPOSED]`.
2. **Do not relitigate §2 decisions.** They are settled, with rationale recorded. If you believe one is wrong, raise it as a single flagged note — do not silently implement an alternative.
3. **Do not expand scope.** §11 is a list of things that must not be built. Adding "small" adjacent features is the most likely way this misses its date.
4. **Blocking questions block.** §10 separates `BLOCKING` (stop, ask) from `DEFAULT` (proceed with the stated assumption, mark the code with a `TODO(spec-Q<n>)`).
5. **Every requirement maps to at least one automated test.** A task producing code without a test for its requirement is incomplete.
6. **Prefer boring.** Standard library first, then one well-established dependency, then write it. Every new dependency is a supply-chain liability (§9) and must be justified in the task.

**Task granularity target:** each task should be completable and independently reviewable in under a day, produce a working increment, and leave the build green. Vertical slices over horizontal layers wherever possible.

---

## 1. System context

```
Chef Infra Client (unmodified, 500–20k nodes)
        │  HTTPS POST, data_collector.token
        ▼
   ┌──────────┐   raw bytes    ┌─────────────────┐
   │  Ingest  │───────────────▶│  Object storage │  (raw archive)
   │   API    │                └─────────────────┘
   └────┬─────┘
        │ enqueue
        ▼
   ┌──────────┐                ┌─────────────────┐
   │  Queue   │───▶ Workers ──▶│   PostgreSQL    │  (hot + warm)
   └──────────┘                └────────┬────────┘
                                        │ export (Parquet + signed manifest)
                                        ▼
                               ┌─────────────────┐
                               │ Customer object │  (cold, arms-length)
                               │    storage      │
                               └─────────────────┘

   ┌──────────┐
   │ Query API│◀── CLI, reference UI, (later) MCP, Grafana
   └────┬─────┘
        │
   ┌────▼─────┐     ┌──────────────────────────────┐
   │  Authz   │◀────│ Identity (Dex): OIDC/SAML/    │
   └──────────┘     │ LDAP/local + API tokens       │
                    └──────────────────────────────┘
```

**Deployment shape:** self-hosted, single-tenant, air-gap-capable. The customer runs everything in the diagram except the customer object storage, which they also run. We ship software, not a service.

---

## 2. Architecture decisions (settled)

| ID | Decision | Rationale | If overridden |
|---|---|---|---|
| **ADR-01** | **Go** | Ops-tooling ecosystem and hiring pool; mature libraries for OIDC, PKCS#11, Parquet, OTel; single static binary; straightforward cross-compile. | Rust is defensible but costs ~20% schedule on library maturity for this specific surface. Escalate before changing — it invalidates ADR-05. |
| **ADR-02** | **PostgreSQL 15+, single engine** | Covers the entire realistic customer band (≤20k nodes) once ingest-time filtering lands. One database to operate, patch, back up, air-gap. | ClickHouse is a v1.1 item triggered by a real >20k opportunity, not roadmap optimism. |
| **ADR-03** | **Versioned HTTP API is the only contract** | CLI, UI, MCP, Grafana are peer clients. No private endpoints, ever. Prevents API rot and is the premise of the whole product. | Not overridable. This is the product thesis. |
| **ADR-04** | **Raw archive written before parsing** | Chain-of-custody root and schema-migration escape hatch. Every derived table is reproducible from it. | Not overridable. |
| **ADR-05** | **Embed Dex for identity federation** | Purpose-built OIDC/SAML/LDAP federation; Automate used it against this same customer base. Saves ~2 eng-months and avoids owning SAML security maintenance. | Adds ~2 eng-months and a permanent security obligation. **Confirm in week one** (§10, Q1). |
| **ADR-06** | **Parquet + zstd for archive export** | Readable by DuckDB, pandas, Spark, ClickHouse, Postgres with no code from us. Evidence readable only through vendor software is weak evidence. | Not overridable — it's a stated differentiator. |
| **ADR-07** | **Ingest-time filtering of no-op resource events** | 95–99% of resource events are `up-to-date`. Persisting them makes aggregates unusable at 20k nodes. Duration signal preserved via rollups. | Not overridable. Reversible in effect via ADR-04 reprocessing. |
| **ADR-08** | **Single-tenant, no tenancy in the schema** | Product is self-hosted; Automate has no true multi-tenancy either. Intra-org project scoping only. | Adding tenancy later is a schema migration, not a rewrite. Acceptable risk. |
| **ADR-09** | **Signing supports local and external key custody in v1** | Both are required by customers. This is a genuine abstraction, not speculative extensibility. | Not overridable. |
| **ADR-10** | **Queue absorbs burst, not the database** | Converge storms produce 5–10x peaks. Ingest must never backpressure into the fleet. | Not overridable. |

**Queue selection is open** — see §10 Q5. Default: Postgres-backed job queue (e.g. River or equivalent) to avoid a second infrastructure dependency in an air-gapped install.

---

## 3. Repository layout

```
/cmd
  /spindle-server        # API + ingest HTTP surface
  /spindle-worker        # queue consumers, rollups, export jobs
  /spindle-cli           # operator + user CLI
/internal
  /ingest            # C1 endpoint, validation, enqueue
  /rawarchive        # C2 verbatim payload store
  /pipeline          # C3 parse, normalize, filter, rollup
  /store             # C4 data-access layer (Postgres)
    /migrations
  /api               # C5 handlers, filter grammar, pagination, errors
  /identity          # C6 Dex integration, connectors, claim mapping
  /tokens            # C7 API token subsystem
  /authz             # C8 policy evaluation, scoping
  /signing           # C9 signer interface, local + PKCS#11 + KMS
  /compliance        # C10 deterministic report generation
  /archive           # C11 Parquet export, manifests, verification
  /obs               # logging, metrics, tracing
/deploy              # packaging, air-gap bundle, migrations runner
/testdata
  /corpus            # captured data collector traffic (see C1)
/docs
  /api               # generated OpenAPI
  /operator          # install, backup, storage requirements
```

**Rule:** `internal/api` may import `internal/store` only through the interfaces in `internal/store`. No SQL outside `internal/store`. No engine-specific constructs surfaced through the API (ADR-02 leaves the ClickHouse door open at near-zero cost, but only if this holds).

---

## 4. Component specifications

Each component lists requirements, dependencies, and acceptance criteria. Requirements are `MUST` unless marked `SHOULD`.

---

### C1 — Ingest endpoint (`ING`)

**Depends on:** C2, C4 (schema), queue.

| ID | Requirement |
|---|---|
| ING-01 | Accept HTTP POST of Chef data collector messages: run-start, run-converge, and compliance report payloads. |
| ING-02 | Authenticate via the existing `data_collector.token` contract. Token compared in constant time. |
| ING-03 | **Schema is defined by captured traffic, not documentation.** Build a recording proxy against a live Automate instance and capture a corpus covering ≥3 Chef client versions, ≥4 platforms, success/failure/partial runs, and compliance-phase runs. Corpus lands in `/testdata/corpus` and is the ingest test suite. |
| ING-04 | Write the verbatim payload to the raw archive (C2) **before** any parsing or validation beyond size limits. |
| ING-05 | Enqueue for async processing, then acknowledge. Endpoint p99 latency under 100ms excluding archive write. |
| ING-06 | Idempotent on message identity. Replaying an identical payload must not produce duplicate rows. Derive the identity key from the corpus in ING-03; document it. |
| ING-07 | Malformed or unparseable payloads are archived, flagged, and counted — never dropped, never 500. Respond 202. |
| ING-08 | Bounded queue depth. On saturation return 429 with `Retry-After`. Never block, never cascade into the fleet. |
| ING-09 | Deployment-wide rate limiting with configurable ceiling. |
| ING-10 | Accept InSpec JSON reporter output posted directly, for scans run outside a converge. |
| ING-11 | Configurable maximum payload size with a clear 413. Default 32MB; validate against corpus. |
| ING-12 | Horizontally scalable — no per-instance state. Multiple ingest processes behind one load balancer must be correct. |

**Acceptance**
- Replay of the full corpus produces zero dropped or misparsed messages.
- Corpus replayed twice produces identical row counts (ING-06).
- Sustained 150 runs/sec with p99 under 100ms on reference hardware.
- Queue saturation test returns 429 and recovers without data loss.

---

### C2 — Raw archive (`RAW`)

**Depends on:** object storage config.

| ID | Requirement |
|---|---|
| RAW-01 | Every accepted payload written verbatim, unmodified, before parsing. |
| RAW-02 | Content-addressed keys; store digest, receipt timestamp, source token identity, declared content type. |
| RAW-03 | Support S3-compatible endpoints (AWS S3, MinIO, others). Configurable endpoint, region, path style. |
| RAW-04 | Local filesystem backend for air-gapped installs with no object store. |
| RAW-05 | Write failure is a hard failure — reject the ingest with 503 rather than accept unarchived data. |
| RAW-06 | Batched or streamed writes are permitted for throughput; batching must not lose the per-payload digest. |
| RAW-07 | Reprocessing API: given a time range, re-emit archived payloads through the pipeline (C3) into a target schema version. |

**Acceptance**
- Kill the process mid-batch; no acknowledged payload is missing from the archive.
- RAW-07 rebuilds a full day of derived tables from archive alone, matching the original within documented tolerances.

---

### C3 — Processing pipeline (`PIPE`)

**Depends on:** C2, C4.

| ID | Requirement |
|---|---|
| PIPE-01 | Parse and normalize payloads into the entities in C4. |
| PIPE-02 | **Persist resource events only where status is `updated`, `failed`, or `skipped`.** No-op events contribute to counts and rollups, then are discarded (ADR-07). |
| PIPE-03 | Maintain per-run status counts covering all events including discarded ones. |
| PIPE-04 | Compute hourly duration rollups keyed by `(cookbook, cookbook_version, resource_type, platform)` with count, sum, p50, p95, p99, max. Computed at ingest, not on a schedule. |
| PIPE-05 | Control results are **never** filtered or rolled up. Full fidelity until export. |
| PIPE-06 | Unknown or unrecognized fields are preserved in a semi-structured column, never silently dropped. |
| PIPE-07 | Processing failures move the message to a dead-letter queue with the original archive reference, and increment an alertable metric. |
| PIPE-08 | Schema version stamped on every derived row to support reprocessing. |
| PIPE-09 | Derive `cookbook_usage` (which cookbook versions ran where, when) as part of run processing. |

**Acceptance**
- Post-filter fleet timing aggregates land within 5% of unfiltered values computed from the same corpus (validates PIPE-04 against PIPE-02).
- Dead-letter path exercised by a deliberately malformed payload; message is recoverable and reprocessable.

---

### C4 — Storage layer (`STO`)

**Depends on:** nothing.

| ID | Requirement |
|---|---|
| STO-01 | Entities: `node`, `run`, `resource_event`, `compliance_report`, `control_result`, `profile`, `waiver`, `cookbook_usage`, plus rollup and audit tables. |
| STO-02 | `resource_event`, `control_result`, `compliance_report`, `run` are declaratively partitioned by day. Automated partition creation ahead of need and detach-on-archive. |
| STO-03 | Node attributes stored as JSONB. Expression indexes on `platform`, `platform_version`, `chef_environment`, policy group only. Do not flatten to columns. |
| STO-04 | BRIN indexes on time columns of high-volume append tables. |
| STO-05 | Append-only semantics on evidence tables. Corrections are new rows; no UPDATE, no DELETE outside the retention job. |
| STO-06 | Hash-chained record sequence per deployment with periodic signed checkpoints (signing via C9). |
| STO-07 | All access through interfaces in `internal/store`. No SQL elsewhere. No engine-specific constructs exposed upward. |
| STO-08 | Forward-only migrations, each independently runnable, with a documented rollback or replay-from-archive path. |
| STO-09 | Retention job: hot→warm transition at 90 days, warm→export at 1 year. Configurable. Deletion requires explicit dual-authorization and is fully audited. |

**Acceptance**
- Load test at 20,000-node volumes: largest partition ~8M rows, aggregate queries within documented budgets.
- Hash chain verifies end-to-end after 24h of synthetic ingest including a process restart.

---

### C5 — Query API (`API`)

**Depends on:** C4, C8.

| ID | Requirement |
|---|---|
| API-01 | Versioned under `/v1/`. Published deprecation policy: minimum two minor versions notice. |
| API-02 | Endpoints: nodes (list/filter/detail/current-state), runs (list/detail with resource events), resource-event aggregates (group by cookbook, resource type, platform; sum and percentile duration), compliance (reports, detail, control results, per-node and per-profile status), waivers (CRUD), drift (resources by update frequency over a window), cookbook inventory, health/meta (ingest lag, queue depth, API version, export/restore job status). |
| API-03 | One filter, sort, and time-range grammar shared by every list endpoint. Documented formally. No endpoint-specific special cases. |
| API-04 | Cursor pagination with deterministic ordering and stable cursors across concurrent writes. |
| API-05 | Query cost limits and timeouts. No unbounded scans. Exceeding the limit returns a structured error naming the offending constraint. |
| API-06 | OpenAPI document generated from the implementation and served by the API itself at a documented path. |
| API-07 | Uniform error envelope: stable machine-readable `code`, human `message`, optional `details`, and a `request_id` present on every response including successes. |
| API-08 | Every response carries the API version and, where derived from rollups or restored archives, a data-provenance marker. |
| API-09 | Authorization evaluated identically for session-backed and token-backed requests — one code path (see AUTHZ-06). |

**Acceptance**
- Contract tests generated from the OpenAPI document pass against a running instance.
- Filter grammar conformance suite passes identically on every list endpoint.
- Cursor stability test: paginate while writing; no duplicates, no skips.

---

### C6 — Identity federation (`IDP`)

**Depends on:** ADR-05 confirmation (§10 Q1).

| ID | Requirement |
|---|---|
| IDP-01 | Single internal identity model (subject, groups/claims, source connector). All connectors federate into it. Authorization operates only on this model. |
| IDP-02 | OIDC connector: authorization code + PKCE. Validated against Okta and Entra ID minimum. |
| IDP-03 | SAML 2.0 connector: SP-initiated and IdP-initiated, signed assertions, encrypted assertions, metadata exchange. |
| IDP-04 | LDAP/AD connector: direct bind, nested group resolution, referral handling, configurable group-cache TTL with manual refresh. |
| IDP-05 | Local accounts for break-glass and bootstrap. Strong password policy, forced rotation, fully audited, disabled by default after initial setup. Must work with no network egress. |
| IDP-06 | Multiple connectors enabled simultaneously. |
| IDP-07 | Configurable rules mapping external groups/claims to internal roles and project scopes, with **documented deterministic precedence** when multiple rules match. No implicit ordering. |
| IDP-08 | JIT user provisioning on first successful login. |
| IDP-09 | **Mapping preview endpoint**: given a claim set, return the roles and scopes that would result, without a login. |
| IDP-10 | Mapping rule changes are audit-logged with before and after state. |
| IDP-11 | Sessions: short-lived access tokens with refresh, configurable idle and absolute timeouts, single-logout where the IdP supports it, admin revocation individually and in bulk. |

**Acceptance**
- Each connector authenticates against a reference IdP in CI (containerized Keycloak or equivalent for OIDC/SAML; OpenLDAP for LDAP).
- IDP-09 predicts roles correctly for every case in the mapping test matrix.
- Break-glass login succeeds with all external connectors unreachable.

---

### C7 — API token subsystem (`TOK`)

**Depends on:** C6, C8.

| ID | Requirement |
|---|---|
| TOK-01 | Three token types: **user-owned** (inherits owner scope, cannot exceed it), **service account** (owned by a non-human principal), **agent** (short TTL default, narrow scope, read-only). |
| TOK-02 | Creation captures name, description, owner, explicit role and project scope, TTL bounded by a policy maximum. |
| TOK-03 | **Plaintext returned exactly once at creation.** Store a hash only (Argon2id or equivalent). Never retrievable afterward. |
| TOK-04 | Revocation: individual, bulk by owner, bulk by scope, bulk by connector. Takes effect on the next request. |
| TOK-05 | Automatic expiry with configurable advance warning to owner and admins. |
| TOK-06 | Rotation with overlapping validity windows so automation rolls without downtime. |
| TOK-07 | Track `last_used_at`. Surface never-used and long-idle tokens in an admin report. |
| TOK-08 | **Reconciliation job**: periodically re-resolve every token owner against its connector; disable orphans; produce an admin report of unresolvable owners. This substitutes for SCIM (§11) and closes the deprovisioning gap. |
| TOK-09 | Every token lifecycle event audited: create, use (sampled or aggregated), rotate, revoke, expire, orphan-disable. |

**Acceptance**
- Owner removed from the directory → token disabled within one reconciliation cycle and appears on the orphan report.
- Revocation terminates authorization on the next request on both session and token code paths.
- Token plaintext appears in no log, no database column, and no error message.

---

### C8 — Authorization (`AUTHZ`)

**Depends on:** C4, C6.

| ID | Requirement |
|---|---|
| AUTHZ-01 | Project/team scoping enforced **at the query layer**, not the presentation layer. |
| AUTHZ-02 | A scoped principal must be unable to observe out-of-scope node counts, aggregates, metadata, or existence — via any endpoint, including error messages and pagination totals. |
| AUTHZ-03 | Minimum roles: `ingest`, `viewer`, `compliance-auditor` (compliance read and export, no node attribute access), `token-admin`, `admin`. |
| AUTHZ-04 | Every decision logged with subject, resource, decision, and the rule that produced it. |
| AUTHZ-05 | Scoping applied by the store layer through a mandatory context parameter — a query that omits scope must fail to compile or fail at runtime, never silently return everything. |
| AUTHZ-06 | One evaluation path for session-backed and token-backed requests. No exceptions, no bypass for internal callers. |

**Acceptance**
- Negative-authorization suite: for every endpoint, a scoped principal receives no out-of-scope data, no count leakage, and no existence disclosure through error differences.
- Static or test-enforced check that no store method can be called without a scope context.

---

### C9 — Signing and key management (`SIG`)

**Depends on:** nothing.

| ID | Requirement |
|---|---|
| SIG-01 | Signer interface with two implementations: **local** (keypair generated at install, private key encrypted at rest under a documented key hierarchy, unlock material via file, env, or operator prompt) and **external** (PKCS#11 HSM, AWS KMS, Azure Key Vault, GCP KMS, Vault Transit). |
| SIG-02 | **Every manifest, export, and checkpoint records the key identifier that signed it.** Non-negotiable. |
| SIG-03 | Historical public keys retained indefinitely. Archives signed by a retired key must remain verifiable years later. |
| SIG-04 | Key rotation without invalidating prior signatures. Rotation is an audit event. |
| SIG-05 | Public keys published in a documented location and format so a third party can verify an archive **with no code from us**. |
| SIG-06 | Signing failure is a hard failure. An artifact that cannot be signed does not ship unsigned. |
| SIG-07 | Key operations rate-limited and audited. |
| SIG-08 | Local mode fully functional air-gapped, with no external dependency. |

**Acceptance**
- Archive signed under key A, after rotation to key B, verifies using retained public key A.
- Verification succeeds using only published keys and an off-the-shelf tool.
- External signer validated against at least one real HSM and one cloud KMS (§10 Q4 selects which).

---

### C10 — Compliance export (`CMP`)

**Depends on:** C4, C8, C9.

| ID | Requirement |
|---|---|
| CMP-01 | Fixed, versioned report definitions. Report type and definition version recorded in every output. |
| CMP-02 | **Deterministic**: same inputs and definition version produce byte-identical output. |
| CMP-03 | Server-side generation only. Never assembled by a client. |
| CMP-04 | Signed with detached attestation covering source data range, definition version, and key identifier. |
| CMP-05 | Reproducible on demand from the raw archive. |
| CMP-06 | Formats: JSON and CSV. |
| CMP-07 | Report types: control-status-by-node, profile-summary-over-time, waiver register, exception/deviation list. |
| CMP-08 | **Not exposed through the MCP adapter** when that ships. Agent-generated compliance evidence is not evidence. |
| CMP-09 | Reports derived from restored archives carry the verification status of the source set. |
| CMP-10 | Every read of compliance data is audit-logged. |

**Acceptance**
- Same report regenerated 30 days later from the raw archive is byte-identical.
- Determinism holds across process restarts, differing row insertion order, and parallel generation.

---

### C11 — Archive export, BYOS (`ARC`)

**Depends on:** C4, C9.

| ID | Requirement |
|---|---|
| ARC-01 | Export to Parquet + zstd, one archive set per weekly time partition. |
| ARC-02 | Documented, versioned schema shipped alongside the data. |
| ARC-03 | Output loads and queries correctly in DuckDB with no code from us. |
| ARC-04 | Signed manifest per set: content hashes, record counts, time range, schema version, source raw-payload digests, export timestamp, signing key identifier. |
| ARC-05 | **Manifests retained in the deployment's own database for the full retention period**, independent of the exported data. Manifests are the chain of custody. |
| ARC-06 | Backup documentation states explicitly that losing manifests is worse than losing archive sets. |
| ARC-07 | Export to a configurable customer-controlled destination (S3-compatible, or written locally for the operator to move). |
| ARC-08 | Verification tool: given an archive set and its manifest, report match or mismatch with specifics. Usable standalone. |
| ARC-09 | Post-export deletion of exported hot/warm rows only after successful export **and** successful verification read-back. |

**Acceptance**
- Exported set loads in DuckDB and row counts match the manifest.
- Deliberately corrupted set fails verification with a specific, actionable diagnostic.
- Export interrupted mid-write leaves no partial set marked complete and no source rows deleted.

---

### C12 — CLI (`CLI`)

**Depends on:** C5.

| ID | Requirement |
|---|---|
| CLI-01 | Full coverage of the v1 API surface. No capability available only in the UI. |
| CLI-02 | `--output json` on every command; JSON is stable and documented. Human-readable is the default for TTY. |
| CLI-03 | Non-interactive by default. No prompts unless `--interactive`. |
| CLI-04 | Named config profiles for multiple endpoints. Credentials never written in plaintext. |
| CLI-05 | Operator subcommands: migrate, export, verify, reconcile-tokens, key rotate, health. |
| CLI-06 | Exit codes distinguish success, user error, auth failure, server error, and partial success. |

---

### C13 — Operations and packaging (`OPS`)

| ID | Requirement |
|---|---|
| OPS-01 | Single binary or single container per role (`server`, `worker`). One config file. |
| OPS-02 | Air-gapped install path with no phone-home required for any operation including licensing. |
| OPS-03 | Health, readiness, ingest lag, queue depth, and dead-letter count exported as Prometheus/OTel. |
| OPS-04 | Documented and **tested** backup and restore procedure covering database and manifests. |
| OPS-05 | Structured logs. No secrets, no token plaintext, no assertion contents. |
| OPS-06 | Upgrade path with migrations that are reversible or replay-safe from the raw archive. |
| OPS-07 | Reference hardware spec published and validated (§10 Q3). |
| OPS-08 | Customer storage requirements document: object-lock/WORM configuration, retention lock, access controls — positioned as the customer's compliance obligation. |

---

## 5. Cross-cutting requirements (`X`)

| ID | Requirement |
|---|---|
| X-01 | Config precedence: flags > env > file > defaults. Every setting documented with type, default, and effect. Invalid config fails at startup with a specific message, never at first use. |
| X-02 | All errors wrapped with context. No bare error returns across package boundaries. |
| X-03 | `request_id` generated at edge, propagated through queue and worker, present in every log line and API response. |
| X-04 | Graceful shutdown: drain in-flight requests, finish or requeue in-flight jobs, close cleanly within a configurable deadline. |
| X-05 | All timestamps stored and returned as UTC, RFC 3339. Timezone conversion is a client concern. |
| X-06 | No panic in production paths. Recover at the top of each goroutine with a logged, alertable metric. |
| X-07 | Test layers: unit for logic; integration against real Postgres and real object storage (containerized); corpus replay for ingest; contract tests from OpenAPI. No mocks for the database. |
| X-08 | Every dependency addition justified in its task, checked for license compatibility and maintenance status. |

---

## 6. Build sequence

Milestones are dependency-ordered. Each must leave the build green and demonstrable.

**M0 — Foundations (week 1–2)**
- ING-03 data capture (corpus capture proxy is now a separate project — not part of Spindle core).
- ADR-05 decision (§10 Q1).
- Repo skeleton, CI, migration runner, containerized test infrastructure.

**M1 — Ingest to storage (week 3–6)**
- C2 raw archive → C1 ingest → C4 core schema → C3 pipeline.
- Demonstrable: corpus replays end to end; rows land; reprocessing works.

**M2 — Query and authorization (week 6–10)**
- C5 query API, C8 authorization, C4 scoping enforcement.
- Demonstrable: scoped queries with negative-authorization suite passing.

**M3 — Identity (week 8–13, parallel with M2)**
- C6 connectors and mapping, C7 token subsystem.
- Demonstrable: all four connectors authenticating in CI; token lifecycle complete.

**M4 — Evidence (week 11–15)**
- C9 signing, C10 compliance export, C11 archive export.
- Demonstrable: deterministic signed report; archive set verified in DuckDB.

**M5 — Delivery (week 14–17)**
- C12 CLI, C13 packaging, air-gap bundle, docs, load test at 20k volumes.

C6/C7 parallelize with C5/C8 if staffing allows — they share only the identity model interface, which should be defined and frozen in M0.

---

## 7. Definition of done for v1

Inherited from PRD §13, restated as testable gates:

1. Corpus replay: zero dropped or misparsed messages.
2. 960,000 runs/day sustained on reference hardware, ingest lag p99 under 60s, headroom to 2x demonstrated.
3. Scoped auditor principal retrieves a signed compliance export and can observe no out-of-scope data by any means.
4. Report regenerated from raw archive 30 days later is byte-identical.
5. Reference UI runs entirely on public endpoints — verified by proxy log audit.
6. Dual-ship demonstrated against a live fleet with no change beyond `client.rb`.
7. Post-filter timing aggregates within 5% of unfiltered values on the same corpus.
8. Exported Parquet loads and queries in DuckDB with no code from us.
9. Corrupted archive set fails verification with actionable diagnostics.
10. Each connector authenticates against a reference IdP; mapping preview predicts correctly across the test matrix.
11. Token whose owner is removed is disabled within one reconciliation cycle and reported.
12. Archive signed under key A verifies after rotation to key B using retained public key A, using published keys alone.
13. External signer validated against one HSM and one cloud KMS; local mode fully air-gapped.
14. Token revocation terminates authorization on next request on both code paths.

---

## 8. Open items

### BLOCKING — stop and ask

| Q | Question | Why blocking |
|---|---|---|
| Q1 | **Approve or reject embedding Dex (ADR-05).** | ±2 eng-months. Determines whether the date holds. Decide in week 1. |
| Q2 | **Data collector message identity key for idempotency (ING-06).** | Cannot be guessed; must come from the ING-03 corpus. Blocks C1 correctness. |

### DEFAULT — proceed with the stated assumption, mark `TODO(spec-Q<n>)`

| Q | Assumption | Impact if wrong |
|---|---|---|
| Q3 | Reference hardware: 16 vCPU / 64GB / NVMe at the 20k ceiling. | Load-test target and published sizing shift. |
| Q4 | Validate PKCS#11 against SoftHSM in CI; real HSM/KMS selection pending a named prospect. | PKCS#11 implementations vary; field surprises possible. |
| Q5 | Queue: Postgres-backed job queue, no separate broker. | If throughput proves insufficient, swap behind an interface. Keep the queue abstraction thin and replaceable. |
| Q6 | Average 300 resource events per converge; ~400 controls per daily scan. | All volume estimates scale proportionally. **Correct from the ING-03 corpus in week 2 and update §6/§7 targets.** |
| Q7 | Hot 90 days, warm to 1 year, export beyond. | Configurable; only defaults change. |
| Q8 | LDAP connector included. | If no early prospect is LDAP-only, this is the first cut under schedule pressure. |

---

## 9. Supply chain requirements

Applies to the build itself, not just the product. Expect this to be attacked in competitive deals.

- Reproducible builds
- Signed release artifacts, published verification keys
- SBOM per release
- Build provenance attestation with a publicly stated SLSA target
- Documented vulnerability disclosure process and patch SLA
- Dependency additions reviewed for license and maintenance status (X-08)

---

## 10. Do not build

These are out of scope for v1. Do not implement, do not stub speculatively, do not add extension points "for later."

- Multi-tenancy of any kind (ADR-08)
- ClickHouse or any second storage engine
- Archive restore-and-query (v1.1 — nothing can need it before year one)
- MCP adapter (v1.1)
- Grafana datasource (v1.1)
- SCIM provisioning (covered by TOK-08)
- ServiceNow / ITSM integration
- Agentless scanning or scan job scheduling
- Historical import from Automate Elasticsearch — **permanently out of scope**
- Notification or alerting engine
- Habitat service groups, applications dashboard
- Any write path toward the managed fleet (remediation, job execution)
- PDF report rendering
- A web UI (specified separately; must consume only public endpoints)

---

## 11. Glossary

| Term | Meaning |
|---|---|
| **Archive set** | One weekly Parquet export plus its signed manifest. |
| **Converge / run** | One Chef Infra Client execution against one node. |
| **Corpus** | Captured production data collector traffic used as the ingest test suite (ING-03). |
| **Control result** | Outcome of one InSpec control on one node in one scan. |
| **Dual-ship** | Client sending reports to both an existing Automate and this service simultaneously. |
| **Manifest** | Signed metadata describing an archive set; the chain-of-custody record. |
| **Raw archive** | Verbatim, pre-parse store of every accepted payload (ADR-04). |
| **Resource event** | One resource's outcome within one run. |
| **Rollup** | Pre-aggregated duration statistics preserving signal from discarded no-op events. |
| **Scope** | Project/team restriction applied to a principal, enforced at the query layer. |

# Spindle — Fleet Telemetry & Compliance Evidence Service
## Minimum Feature Set (v1)

> **Codename `Spindle` is pending trademark clearance and domain availability.** Note that "spindle" has prior technical use in disk storage; the trademark class differs but search results will be noisy. Must not contain "Chef" or any Progress mark.

**Target:** shippable pilot by December. Backend and API only; reference UI specified separately.

**Design rule:** the versioned HTTP API is the single contract. CLI, MCP, Grafana, and the reference UI are all peer clients built strictly on public endpoints. No private endpoints, ever.

---

## 1. Scope

**In v1**
- Ingest from unmodified Chef Infra Client and InSpec (no node changes required)
- Durable, queryable store for converge and compliance data
- Immutable evidence retention with deterministic, signed export
- Versioned query API with complete authorization
- CLI and MCP adapters over that API

**Explicitly not in v1** — see §11.

**Primary success condition:** a prospect can dual-ship from their production fleet to us alongside their existing Automate, with two lines in `client.rb` and no other change.

---

## 2. Ingest

### 2.1 Data collector endpoint
- HTTP endpoint accepting Chef data collector messages: run-start, run-converge, and compliance report payloads
- Token authentication matching the existing client configuration contract (`data_collector.token`)
- **Compatibility is defined by captured live traffic, not by docs.** Build a recording proxy against a real Automate instance in week one; the captured corpus is the ingest test suite.

### 2.2 Ingest guarantees
- Accept-and-acknowledge fast; process asynchronously via durable queue
- Idempotent on message identity — replays must not duplicate
- Malformed or unparseable payloads are archived raw and flagged, never dropped
- Backpressure: bounded queue with explicit 429 and retry-after; must not cascade failure into the fleet
- Ingest rate limiting and quota accounting (deployment-wide — single-tenant, so no per-tenant partitioning)

### 2.3 Raw archive (foundational)
- Every accepted payload is written verbatim to immutable object storage before any parsing
- All derived tables are reprocessable from the raw archive
- This is the evidence chain-of-custody root and the schema-migration escape hatch. Non-optional.

### 2.4 Direct InSpec ingest
- Accept InSpec JSON reporter output posted directly, for scan jobs run outside a converge

---

## 3. Data model

Minimum entities:

| Entity | Notes |
|---|---|
| `node` | Identity, platform, last-seen, current attributes, tenant/project assignment |
| `run` | Converge run: status, start/end, duration, cookbook set, error summary |
| `resource_event` | Per-resource: type, name, action, result (up-to-date/updated/failed/skipped), duration, cookbook + version, guard outcome |
| `compliance_report` | Scan instance: profile set, node, timestamp, summary counts |
| `control_result` | Per-control: id, status, impact, evidence text, waiver reference |
| `profile` | Profile identity + version as executed |
| `waiver` | Control, scope, justification, approver, expiry |
| `cookbook_usage` | Derived: which cookbook versions ran on which nodes when |

### 3.1 Volume targets (confirmed range)

Customers span 500 to 20,000 nodes typical, 150,000 maximum. At a 30-minute interval:

| Fleet | Runs/day | Resource events/day (~300/run) | Control results/day (~400/scan) | Raw archive/day (compressed) |
|---|---|---|---|---|
| 500 | 24,000 | 7.2M | 200K | ~1GB |
| 5,000 (pilot) | 240,000 | 72M | 2M | ~8–15GB |
| 20,000 | 960,000 | 288M | 8M | ~35–60GB |
| 150,000 | 7.2M | 2.16B | 60M | ~250–450GB |

The top of the range is a 300x spread from the bottom. **No single storage configuration serves both ends well**, so the storage engine sits behind an interface and ships in two profiles.

### 3.2 Storage decision

**PostgreSQL, declaratively time-partitioned. Single engine in v1.**

Even customers who eventually reach 150,000 nodes roll out at 10,000–20,000 first, so the Scale tier is many months away. With ingest-time filtering (§3.3) doing the heavy lifting, Postgres comfortably covers 20,000 nodes and has real headroom above it:

| Table at 20,000 nodes | Rows/day | 90-day hot |
|---|---|---|
| Runs | 960K | ~86M |
| Resource events (post-filter, 1–5% persisted) | 3–15M | 270M–1.3B |
| Control results | 8M | ~720M |

Daily partitions keep the largest table at ~8M rows per partition. That is ordinary Postgres, not heroics.

**On the ClickHouse abstraction:** keep a clean data-access layer — normal good architecture, and it costs nothing extra. **Drop the dual-backend conformance suite and the ClickHouse stub from v1.** Building a plugin interface for a backend nobody needs for a year is speculative work; the discipline of not leaking engine specifics into the API is a code-review rule, not a framework. Revisit when a real Scale opportunity is on the board, and expect ~3 eng-months at that point.

Requirements:
- Time-partitioned resource event and control result tables, partition-per-day, automated detach-and-archive
- Node attributes as JSONB with selective expression indexes on the few attributes people actually filter on (platform, platform_version, chef_environment, policy group). Do not flatten to columns.
- BRIN indexes on time columns; avoid btree bloat on high-volume append tables
- No engine-specific constructs surfaced through the query API — no raw SQL passthrough, no Postgres-specific filter operators in the public grammar

### 3.3 Resource event handling (required, not an optimization)

In steady state, 95–99% of resource events are `up-to-date` no-ops. At 20,000 nodes that is ~285M throwaway rows per day.

**Filter at ingest, not on a schedule:**
- Persist full detail only for events with status `updated`, `failed`, or `skipped`
- No-op events contribute to per-run status counts and to duration rollups, then are discarded from the hot store
- **Duration data survives via rollup:** hourly aggregates of duration percentiles keyed by `(cookbook, cookbook_version, resource_type, platform)`, computed at ingest. This preserves the fleet-wide performance question ("which resource costs 90s across the fleet") without retaining the rows.

The raw archive (§2.3) remains complete regardless, so every decision here is reversible by reprocessing.

### 3.4 Ingest scaling

20,000 nodes averages ~11 runs/sec, with converge storms and cron alignment producing 5–10x peaks. Ingest workers scale horizontally against a shared queue, with the queue — not the database — absorbing burst. Load-test at 150 runs/sec sustained.

Design for horizontal ingest even though a single worker would suffice today; that is the piece that must not be rearchitected when a customer grows past 20,000.

---

## 4. Retention & evidence integrity

**Confirmed: 3-year evidence retention, with long-term custody held by the customer.**

| Tier | Window | Contents | Custody |
|---|---|---|---|
| Hot | 0–90 days | Full queryable detail | Deployment's Postgres |
| Warm | 90 days – 1 year | Run summaries, all control results, rollups | Deployment's Postgres, compressed partitions |
| **Cold** | **1–3 years** | **Exported archive sets** | **Customer's object storage of choice** |

Note this is a **self-hosted, single-tenant product**. The customer runs the deployment; we ship software, not a service. That shapes the integrity model below.

### 4.1 Customer-held archive (BYOS)

Export, then hand off. This removes the long-term storage burden entirely — we never touch the bytes.

**Export format — must be readable without our software.**
- Parquet, zstd-compressed, one archive set per time partition (weekly)
- Documented, versioned schema shipped alongside the data
- Loadable directly into DuckDB, pandas, Spark, ClickHouse, or Postgres by anyone
- Evidence an auditor can only read through a vendor's product is weak evidence. Also a real differentiator against Automate — say so in the collateral.

**Signed manifest per archive set, retained in the deployment's own database.**
- Manifest carries: content hashes, record counts, time range, schema version, source raw-payload digests, export timestamp
- Retained for the full 3 years even though the data is discarded — manifests are kilobytes, so three years is a few hundred MB. Small enough to live in the primary database and ride along on normal backups.
- The manifest set is the chain of custody. **Losing it is worse than losing an archive set**, so call it out explicitly in the backup documentation.

### 4.2 Trust model — be honest about what signing proves

Self-hosted means the signing key is under customer control. That is a weaker attestation than a SaaS product could offer, and it should be stated plainly rather than glossed:

- Signatures **do** prove: the archive has not been corrupted in storage or transit, and has not been altered by a third party without the deployment key
- Signatures **do not** prove: that the customer's own administrators did not alter data before export

For most compliance regimes this is fine — auditors already assume the auditee operates the systems generating the evidence, which is what an audit is for. But do not let sales imply otherwise.

**Optional hardening for customers who want more**, ship as v1.1:
- RFC 3161 trusted timestamps on manifest signatures
- Publishing manifest root hashes to an external transparency log or the customer's own append-only notary
- HSM or KMS-backed deployment key with separated administrative duty

### 4.3 The warranty boundary — state it explicitly in contracts

| We warrant | We do not warrant |
|---|---|
| Export is complete and correct for the stated range | Durability of customer-held archives |
| Manifest is accurate and signed | Availability or retrievability |
| Re-import verifies against the retained manifest, loudly | That the customer's storage is WORM-configured |
| Any mismatch is detected and reported | That data has not been deleted |

Ship a **customer storage requirements document** covering object-lock / WORM configuration, retention lock periods, and access controls, positioned as *their* compliance obligation. Auditors accept this — it is how offsite tape custody has worked for decades — but only if the boundary is documented rather than implied.

### 4.4 Restore-and-query

- Async restore job: operator supplies archive set(s), the deployment verifies against its retained manifest, loads into an ephemeral read-only namespace
- Queryable through the normal API via a session identifier, so every existing client works unchanged
- TTL'd, auto-expiring, with disk usage attributed to the session and surfaced in ops metrics
- Selective restore at partition granularity plus filter pushdown ("Q3 2024, nodes matching `platform:windows`")
- **Verification result is part of session metadata** and surfaces on every report generated from restored data. A set that fails verification is still queryable, but every export derived from it is stamped unverified.
- Restore requires free disk headroom; document the sizing formula and fail the job early rather than filling the volume

### 4.5 Hot/warm integrity

- Append-only on all evidence tables; corrections are new records, never mutations
- Hash-chained record sequence per deployment, with periodic signed checkpoints
- Compliance control results are never subject to §3.3 filtering — full fidelity until export
- Deletion only via explicit, logged, dual-authorized retention job
- Full audit log of every read of compliance data

---

## 5. Query API

Versioned (`/v1/`), cursor-paginated, consistent filter grammar across all collections.

**Minimum endpoints**
- Nodes: list, filter, detail, current state
- Runs: list by node/time/status, detail with full resource event set
- Resource events: aggregate query — group by cookbook, resource type, platform; sum/percentile duration
- Compliance: report list, report detail, control results, per-node and per-profile status
- Waivers: CRUD
- Drift: resources by update-frequency over a window (the "converging repeatedly" signal)
- Cookbook inventory: which versions are running where
- Health/meta: ingest lag, queue depth, API version, archive/restore session status

**Requirements**
- Every list endpoint supports the same filter, sort, and time-range syntax
- Deterministic ordering with stable cursors
- Query cost limits and timeouts; no unbounded scans
- Machine-readable schema/OpenAPI document served by the API itself
- Published deprecation policy from the first release — minimum two minor versions notice

---

## 6. Compliance export (protected path)

Separate from ad-hoc query. **Must not be agent-generated.**

- Fixed, versioned report definitions (report type + definition version recorded in output)
- Deterministic: same inputs and definition version produce byte-identical output
- Server-side generation only
- Signed (cosign or equivalent) with detached attestation covering the source data range and definition version
- Reproducible on demand from the raw archive
- Formats: JSON and CSV minimum; PDF deferrable to v1.1
- Minimum report types: control-status-by-node, profile-summary-over-time, waiver register, exception/deviation list

---

## 7. Identity, AuthN / AuthZ

**Single-tenant deployment — no tenant isolation required.** No tenant column, no cross-tenant partitioning. What remains is enterprise identity federation plus intra-org scoping, and all of it is v1: an open API has no UI layer to hide authorization behind, and retrofitting scoping breaks every client customers build on it.

### 7.1 Architecture — one internal identity model, pluggable connectors

Do **not** implement three parallel auth stacks. Federate everything into a single internal identity representation (subject, groups/claims, source connector), and let authorization operate only on that.

**Strongly consider embedding Dex** (CNCF, Go) rather than writing connectors. It is purpose-built for exactly this — an OIDC provider that federates upstream to LDAP, SAML, OIDC, and others — and Chef Automate itself used it, so the pattern is proven against this customer base. Estimated saving: ~2 eng-months versus building connectors, at the cost of one additional embedded component.

### 7.2 Required connectors (all v1)

| Connector | Notes |
|---|---|
| **OIDC** | Okta, Entra ID, Ping, Auth0, Google Workspace. Standard authorization-code + PKCE. |
| **SAML 2.0** | Promoted from deferred. Non-negotiable for this buyer segment — regulated, long-lived estates skew SAML. Must handle IdP-initiated and SP-initiated, signed assertions, encrypted assertions, and metadata exchange. |
| **LDAP / Active Directory** | Direct bind. Many air-gapped and regulated shops have AD and no modern IdP at all. Must handle nested group resolution (slow in AD — cache with configurable TTL) and referrals. |
| **Local accounts** | Break-glass and bootstrap. Required for air-gapped install and for recovery when the IdP is unreachable. Forced rotation, strong password policy, fully audited, disabled by default after initial setup. |

Multiple connectors must be enabled simultaneously — an org may run Okta for staff and LDAP for service identities.

### 7.3 Group and claim mapping

This is the part that generates support tickets, not the protocol work.

- Configurable rules mapping external groups/claims → internal roles and project scopes
- Deterministic, documented precedence when multiple rules match; no implicit ordering
- Group membership caching with configurable TTL, plus manual refresh
- JIT user provisioning on first successful login
- **A mapping preview endpoint**: "given this set of claims, what roles and scopes would result?" Test it without making a user log in. This single feature will pay for itself in support load.
- Mapping changes are audit-logged with before/after

### 7.4 Sessions

- Short-lived access tokens with refresh; configurable idle and absolute timeouts
- Single-logout support where the IdP provides it
- Session revocation by admin, individually and in bulk

### 7.5 API token management

A first-class subsystem, not a side effect of the auth work.

**Token types**
- **User-owned tokens** — inherit the owner's scope, cannot exceed it, die when the owner is deprovisioned
- **Service account tokens** — owned by a service account, not a person. Required: user-owned tokens breaking when an employee leaves is a real outage and a real audit finding.
- **Agent tokens** — short TTL by default, narrow scope, read-only. Ties to §9: an agent querying on a user's behalf gets least privilege by construction.

**Lifecycle**
- Creation: name, description, owner, explicit role + project scope selection, TTL with a policy-enforced maximum
- **Plaintext shown exactly once at creation.** Store a hash only; never retrievable afterward.
- Revocation: individual, bulk by owner, bulk by scope, bulk by connector
- Expiry: automatic, with configurable advance warning to owner and admins
- Rotation: overlapping validity windows so automation can roll without downtime
- `last_used_at` tracked per token; surface never-used and long-idle tokens for cleanup

**Deprovisioning gap — flag this explicitly**
Without SCIM (deferred, §11), the system learns about departures only at next login. Mitigations required in v1:
- Short session TTLs
- Periodic reconciliation job that re-resolves every token owner against the connector and disables orphans
- An admin report of tokens whose owner no longer resolves

This is an audit finding waiting to happen if left unaddressed, and the reconciliation job is much cheaper than SCIM.

### 7.6 Authorization

- Project/team scoping enforced **at the query layer**, not the presentation layer — a scoped token must be unable to observe out-of-scope node counts, aggregates, or metadata
- Minimum roles: ingest, viewer, compliance-auditor (compliance read + export, no node attribute access), token-admin, admin
- Every authorization decision logged with subject, resource, decision, and the rule that produced it
- Authorization is evaluated identically for session-backed and token-backed requests — one code path, no exceptions

---

## 8. Signing key management

Both custody models are required in v1, so this is a genuine abstraction rather than speculative extensibility: a signer interface with local and external implementations.

### 8.1 Custody options

| Mode | Detail |
|---|---|
| **Self-generated** | Keypair generated at install. Private key encrypted at rest under a documented key hierarchy; unlock material supplied at service start (file, env, or operator prompt). Default for air-gapped and smaller deployments. |
| **External** | PKCS#11 for HSM; cloud KMS (AWS KMS, Azure Key Vault, GCP KMS); HashiCorp Vault Transit. Private key never enters process memory — sign operations are delegated. |

### 8.2 Requirements common to both

- **Every manifest and export records the key identifier that signed it.** Non-negotiable.
- Historical public keys retained indefinitely so archives signed by a retired key remain verifiable. This is the detail most implementations miss and it breaks year-three verification.
- Key rotation supported without invalidating prior signatures; rotation is an audit event
- Public keys published in a documented location and format so an auditor can verify an archive **without our software** — same principle as the Parquet decision in §4.1
- Signing failures are hard failures: an export that cannot be signed does not ship unsigned
- Key operations rate-limited and audited (relevant for KMS cost and for detecting abuse)

---

## 9. Adapters

Thin, no business logic, no privileged access.

- **CLI** — v1. Full API coverage, `--output json` on everything, non-interactive by default, config profiles for multiple endpoints
- **MCP server** — deferred to v1.1 under current scope. When built: read-only, exposes node/run/compliance queries and schema introspection, and explicitly does **not** expose the compliance export path. Agent tokens per §7.5.
- **Grafana datasource plugin** — deferred to v1.1. Cheap, and reaches more of the target market than MCP does; first thing to restore if schedule allows.

---

## 10. Operations

- Single-binary or single-container deployment; one database, one object store, one config file
- Air-gapped install path (no phone-home required for operation)
- Health, readiness, and ingest-lag metrics exported as Prometheus/OTel
- Backup and restore procedure, documented and tested
- Structured logs, no secrets in logs
- Upgrade path with schema migration that is reversible or replay-safe from the raw archive

---

## 11. Supply chain assurance

Entry requirement, not a differentiator — expect this to be attacked in every competitive deal.

- Reproducible builds
- Signed release artifacts (cosign), published public keys
- SBOM per release
- Build provenance attestation (SLSA level target stated publicly)
- Documented vulnerability disclosure and patch SLA

---

## 12. Explicitly deferred

Named here so they don't creep in:

- ServiceNow / ITSM integration
- Agentless scanning and scan job scheduling
- Historical import from existing Automate Elasticsearch — **confirmed not required; treat as permanently out of scope, not deferred.** Sales position: customer retains their existing Automate cluster read-only for the remainder of its retention window; our clock starts at cutover.
- **SCIM provisioning** — deprovisioning gap covered by the reconciliation job in §7.5. Revisit when a customer demands it contractually.
- **Archive restore-and-query (§4.4)** — deferred to v1.1. Nothing enters the customer-held archive until day 90 at the earliest, and realistically not until year one, so no v1 customer can need this. Export must still be correct and verifiable in v1; only the read-back path slips.
- ClickHouse Scale profile
- Notification/alerting engine
- Habitat service groups, applications dashboard
- Anything write-path toward the fleet (remediation, job execution)
- PDF report rendering

---

## 13. Definition of done for v1

1. Ingests a captured production traffic corpus with zero dropped or misparsed messages
2. Sustains 960,000 runs/day (20,000 nodes @ 30min) on documented Standard-profile reference hardware, ingest lag under 60s at p99, with demonstrated headroom to 2x
3. A scoped auditor token can retrieve a signed compliance export and cannot observe out-of-scope nodes by any endpoint or aggregate
4. An export regenerated from the raw archive 30 days later is byte-identical
5. Reference UI runs entirely on public API endpoints — verified by proxy log audit
6. Dual-ship demonstrated against a live customer fleet with no change beyond `client.rb`
7. Post-filter fleet timing queries return results within 5% of the unfiltered values on the same corpus
8. An archive set exported at month 12 and restored at month 30 verifies against its retained manifest and is queryable through the standard API
9. Exported Parquet loads and queries correctly in DuckDB using no code from us
10. A deliberately corrupted archive set fails verification, is flagged in session metadata, and every export derived from it carries the unverified stamp
11. Each connector (OIDC, SAML, LDAP, local) authenticates against a reference IdP, and the mapping preview endpoint predicts the resulting roles correctly for every test case
12. A token whose owner is removed from the directory is disabled by the reconciliation job within one cycle and appears on the orphan report
13. An archive signed under key A, after rotation to key B, still verifies using the retained public key A — and verifies using published keys with no code from us
14. External signer works against at least one HSM (PKCS#11) and one cloud KMS; self-generated mode works fully air-gapped
15. Revoking a token terminates in-flight authorization on the next request, on both session and token code paths

---

## 14. Rough sizing

| Area | Est. |
|---|---|
| Ingest + raw archive + queue + horizontal workers | 2 eng-months |
| Postgres schema + partitioning + data-access layer | 1.5 |
| Ingest-time filtering + duration rollups | 1 |
| Query API + filter grammar | 2 |
| **Identity federation (Dex-embedded) + claim mapping** | **2** |
| **API token subsystem + lifecycle + reconciliation** | **1.5** |
| **Authorization + project scoping** | **1.5** |
| **Signing key management (local + PKCS#11 + KMS)** | **1.5** |
| Hot/warm integrity, manifests, export | 1.5 |
| Compliance export path | 1.5 |
| CLI adapter | 0.5 |
| Ops, packaging, air-gap, supply chain | 1.5 |
| **Total** | **~18 eng-months** |

Up 2.5 from the previous pass. Identity and key custody added ~4.5; deferring archive restore (~1) and the MCP/Grafana adapters (~1) gave half of it back.

**Embedding Dex is what makes this fit.** Writing OIDC, SAML, and LDAP connectors from scratch is ~2 additional eng-months and a permanent security-maintenance obligation. If Dex is rejected for any reason, add that back and expect December to slip.

Five engineers, roughly four months. Tight but achievable.

**Deferred:** archive restore-and-query (~1), MCP adapter (~0.5), Grafana datasource (~0.5), ClickHouse Scale profile (~3).

**If it slips further, cut in this order:** LDAP connector (if no early prospect needs it) → drift queries → cookbook inventory endpoint. Do **not** cut §7.6 scoping, §8's key-identifier recording, or §4.3's warranty boundary.

---

## 15. Resolved

1. **Data collector endpoint remains unrestricted.** Ingest contract is stable; dual-ship land motion holds.
2. **Self-hosted, single-tenant.** No multi-tenancy. Drives §7 and the trust model in §4.2.
3. **Fleet range: 500–20,000 in practice.** Even eventual 150,000-node customers roll out at 10,000–20,000 first. Postgres only in v1 (§3.2).
4. **Retention: 3 years, customer-held beyond year one.** Drives §4.1–4.4.
5. **No historical import required.** Permanently out of scope (§12).
6. **Both key custody models required in v1** — self-generated and external HSM/KMS (§8).
7. **Full enterprise identity stack in v1** — OIDC, SAML, LDAP/AD, local break-glass, plus a managed API token subsystem (§7).

### Still open

1. **Dex embed — approve or reject early.** It is the difference between ~2 and ~4 eng-months on identity, and it changes whether December is achievable. Decide in week one, not month two.
2. **Average resource count per converge.** Assumed 300. Pull the real figure from the week-one traffic capture; it scales every estimate in §3.1 proportionally.
3. **Scan cadence and profile count.** Assumed one daily scan at ~400 controls. CIS *and* STIG *and* custom could be 3–4x, making compliance the dominant storage cost.
4. **Which HSM and KMS to validate against first.** PKCS#11 is a standard with famously inconsistent implementations; pick the one an early prospect actually runs rather than testing against SoftHSM and discovering the gap in the field.
5. **Reference hardware.** Starting proposal: 16 vCPU / 64GB / NVMe at the 20,000-node ceiling. Goes in both the sales collateral and the install guide, so validate early.
6. **Does any early prospect actually run LDAP-only?** If every target has Okta or Entra, the LDAP connector becomes the first thing to cut under schedule pressure. Ask during discovery.

Items 1 and 6 are schedule levers. Item 2 is cheap and unblocks the volume model.

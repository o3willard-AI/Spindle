# Spindle Phase 2 — Integration & Deployment

## Role Assignments

| Agent | Model | Role |
|---|---|---|
| Sergey | deepseek-v4-flash | Release Engineer + Integration Lead |
| Mark | qwen3.7-flash | Deployment Engineer + UAT Lead |
| Mike | laguna-s-2.1 | Core Developer (stub replacement) |

## Infrastructure

| Service | IP | Status |
|---|---|---|
| PostgreSQL 16.14 | 192.168.101.101 | ✅ Deployed, zero migrations |
| Cinc Server 15.10.114 | 192.168.101.220 on .155 | ✅ Running, untested with Spindle |
| Cinc Clients (fleet) | 192.168.101.211-213 on .155 | ✅ Running, never connected |
| Clubhouse services | 192.168.101.42 | ✅ LiteLLM, Qdrant, Mem0 |

---

## Sergey — Release Engineering & Integration

### Integration Task 1: Database Migration & Validation
- Run all 21+ migrations against the live PostgreSQL server (192.168.101.101)
- Verify every table, index, constraint, and partition exists
- Run `sqlx prepare` for compile-time query checking
- Create a `DATABASE_URL` configuration for the Spindle config
- Document: connection string, migration procedure, rollback path

### Integration Task 2: Cinc Server Connectivity
- SSH to Cinc Server (192.168.101.220)
- Verify Cinc Server API is reachable
- Configure Spindle with Cinc Server endpoint
- Test data-collector POST to Spindle ingest from a Cinc Client
- Verify raw archive stores the payload
- Verify pipeline processes it into store tables
- Document: full data flow from Cinc Client → Spindle → queryable data

### Integration Task 3: Dex Identity Sidecar
- Deploy Dex on a VM or Clubhouse as Spindle's identity provider
- Configure OIDC, SAML, and LDAP connectors in Dex config
- Wire Spindle's `DexClient` to the live Dex instance
- Test OIDC login flow end-to-end
- Test JIT user provisioning against live DB
- Document: Dex deployment, connector config, auth flow verification

### Integration Task 4: End-to-End Pipeline Test
- Start with a Cinc Client converge
- Trace the payload through every stage:
  1. POST /ingest/events/data-collector → 202
  2. Raw archive stored on disk/S3
  3. Pipeline parses → normalizes → filters
  4. Store tables populated (nodes, runs, resource_events)
  5. API queries return correct data
  6. Compliance report generated and verified
- Document the full trace with timestamps

### Release Task 5: Build & Package
- `cargo build --release` all three binaries
- Verify static linking (musl target)
- Build `spindle-bundle.tar.gz` with all artifacts
- Test install on a clean VM from the bundle
- Document: release versioning, bundle contents, install steps

---

## Mark — Deployment Engineering & UAT

### UAT Task 1: Acceptance Criteria Validation
- Load the original 14 acceptance criteria from the PRD
- For each criterion, write a test script or manual procedure
- Execute every test against the live deployment
- Record pass/fail with evidence (API response, query result, log line)
- Document in `docs/uat/acceptance-report.md`

### UAT Task 2: Performance & Load Testing
- Run `spindle-bench` against the live deployment
- Validate p99 ingest lag < 60s at 150 req/s
- Validate queue recovery from saturation
- Validate zero data loss under load
- Test at 2x target (300 req/s) for graceful degradation
- Document: benchmarks vs. acceptance criteria, tuning recommendations

### UAT Task 3: Security Audit
- Test every endpoint with invalid/missing tokens → 401
- Test role boundary enforcement (ingest token can't query, viewer can't write)
- Test scope enforcement (project A cannot see project B data)
- Test auditor attribute stripping
- Test timing-safe token comparison
- Document: `docs/uat/security-audit.md`

### UAT Task 4: Air-Gap Deployment
- Provision a clean air-gapped VM (no internet)
- Install from `spindle-bundle.tar.gz`
- Start all services
- Run end-to-end corpus replay
- Verify zero outbound connection attempts (firewall audit)
- Document: air-gap install procedure, verification steps

### UAT Task 5: Third-Party Verification
- Use `tools/verify_spindle_archive.py` against a real archive export
- Verify manifest signature with published JWK keys
- Verify archive against DuckDB
- Document: third-party verification procedure for auditors

### UAT Task 6: Backup & Restore
- Take a live backup (DB dump + archive sync)
- Wipe the database and storage
- Restore from backup
- Verify byte-identical compliance export to pre-backup
- Document: backup/restore runbook with timings

### Deployment Task 7: Documentation Audit
- Verify every doc matches live behavior:
  - `docs/operator/backup-restore.md`
  - `docs/operator/storage-requirements.md`
  - `docs/install-airgap.md`
  - `BENCHMARKS.md`
- Fix any discrepancies found
- Add any missing operational docs

---

## Pre-Task Checklist (applies to ALL tasks)

1. `git pull --rebase` — integrate latest before starting
2. `cargo test` — must be green before and after changes
3. `git status` — clean working tree
4. `git push` — every change lands on origin
5. Post results to Matrix with `[DONE]` tag

## Last Updated
2026-08-08

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
| PostgreSQL 16.14 | 198.51.100.101:5432 | ✅ Deployed, 0 migrations applied |
| Twin-Write Proxy | 198.51.100.101:8081 | ✅ Running, systemd-enabled |
| Spindle (future) | 198.51.100.101:8080 | ⬜ Not yet deployed |
| Cinc Server 15.10.114 | 198.51.100.220:443 | ✅ Running on .155 |
| Cinc Clients (QA fleet) | 198.51.100.211-213 | ✅ Running on .155 |

---

## Data Population Plan — Twin-Write Proxy

### Architecture

```
Cinc Client (211-213)                  Twin-Write Proxy (101:8081)
    │                                        │
    │ POST /ingest/events/data-collector     │
    ├───────────────────────────────────────►│
    │                                        ├──► Spindle (101:8080)
    │                                        │    └─ 202 or error logged
    │                                        │
    │                                        └──► Cinc Server (220:443)
    │                                             └─ original data-collector flow
    │
    │ Operator: GET /health → dashboard
    │   {"spindle": {"success": 5234, "failure": 0, ...},
    │    "cinc_server": {"success": 5234, ...}}
    │
    │ When confidence gained:
    │   Cut over client.rb → SPINDLE_URL directly
```

### Step 1: Apply SQL Migrations
```bash
# Sergey: Run all 21+ migrations against live DB
cd ~/workspace/Spindle
DATABASE_URL=postgres://spindle:spindle-dev-password@198.51.100.101:5432/spindle
sqlx migrate run
# Verify every table, index, partition exists
```

### Step 2: Deploy Spindle Server
```bash
# Sergey: Build and deploy Spindle on 198.51.100.101:8080
cargo build --release -p spindle-server
# Copy binary, config, migrations to 198.51.100.101
# Start with systemd, verify GET /health returns 200
```

### Step 3: Configure QA Fleet for Twin-Write
```bash
# Mark: On each Cinc Client (211-213), add to /etc/cinc/client.rb:
data_collector['server_url'] = 'http://198.51.100.101:8081/ingest/events/data-collector'
data_collector['token'] = 'spindle-dev-token'
```

### Step 4: Begin Data Population
```bash
# On each client VM (211, 212, 213):
sudo cinc-client --once
```

### Step 5: Verify Twin-Write Health
```bash
# Monitor the dashboard:
curl -s http://198.51.100.101:8081/health | python3 -m json.tool
# Watch spindle.success == cinc.success
```

### Step 6: Cut Over
```bash
# When operators trust Spindle after N hours/days of twin-write:
# Update client.rb on each Cinc Client:
data_collector['server_url'] = 'http://198.51.100.101:8080/ingest/events/data-collector'
```

### Continuous Load Generation
```bash
# Sergey: Create a cron job on each QA client for ongoing data:
# /etc/cron.d/spindle-qa — runs cinc-client every 30 min
*/30 * * * * root /usr/bin/cinc-client --once > /dev/null 2>&1
```

---

## Sergey — Release Engineering & Integration

### Integration Task 1: Database Migration & Validation
- Run all 21+ migrations against live PostgreSQL (198.51.100.101)
- Verify every table, index, constraint, and partition
- Run `sqlx prepare` for compile-time query checking
- Add `DATABASE_URL` to Spindle config
- Document: connection string, migration procedure, rollback path

### Integration Task 2: Deploy Spindle Server
- Build `spindle-server` release binary
- Deploy to 198.51.100.101:8080
- Create systemd service
- Verify `GET /health`, `GET /metrics`, `GET /ready`
- Verify ingest endpoints accept payloads
- Verify twin-write proxy forwards correctly to Spindle

### Integration Task 3: Cinc Server Connectivity
- Verify Cinc Server (198.51.100.220) is reachable
- Test twin-write proxy → Cinc Server forwarding
- Configure QA fleet Cinc Clients for twin-write
- Trigger converges on all three clients
- Trace payload through the full pipeline

### Integration Task 4: Dex Identity Sidecar
- Deploy Dex on Clubhouse or a dedicated VM
- Configure OIDC, SAML, LDAP connectors
- Wire Spindle's DexClient to live Dex
- Test full OIDC login flow with JIT provisioning against live DB

### Integration Task 5: End-to-End Pipeline Trace
- Cinc Client converge → twin-write proxy → Spindle ingest → raw archive → pipeline → store tables → API query
- Document timestamps at each stage
- Verify data integrity at each hop

### Release Task 6: Build & Package
- `cargo build --release` all three binaries
- Verify static linking (musl)
- Build `spindle-bundle.tar.gz`
- Test clean install from bundle
- Document release versioning and bundle contents

---

## Mark — Deployment Engineering & UAT

### UAT Task 1: Acceptance Criteria Validation
- Load the 14 original acceptance criteria
- For each, write and execute a test against live deployment
- Record pass/fail with evidence
- Document in `docs/uat/acceptance-report.md`

### UAT Task 2: Performance & Load Testing
- Run `spindle-bench` against live deployment
- Validate p99 ingest lag < 60s at 150 req/s
- Validate queue recovery from saturation
- Validate zero data loss under load
- Test at 2x target (300 req/s)
- Document benchmarks vs. acceptance criteria

### UAT Task 3: Security Audit
- Test every endpoint with invalid/missing tokens → 401
- Test role boundary enforcement
- Test scope enforcement (cross-project data isolation)
- Test auditor attribute stripping
- Test timing-safe token comparison
- Document in `docs/uat/security-audit.md`

### UAT Task 4: Air-Gap Deployment
- Provision clean air-gapped VM
- Install from `spindle-bundle.tar.gz`
- Run end-to-end corpus replay
- Verify zero outbound connections (firewall audit)

### UAT Task 5: Third-Party Verification
- Export a real archive from live data
- Verify manifest signature with published JWK keys
- Load Parquet files in DuckDB
- Run `tools/verify_spindle_archive.py`

### UAT Task 6: Backup & Restore
- Take live backup (DB dump + archive sync)
- Wipe database and storage
- Restore from backup
- Verify byte-identical compliance export

### UAT Task 7: Documentation Audit
- Verify every doc matches live behavior
- Fix discrepancies
- Add missing operational docs

---

## Mike — Stub Replacement Tasks
See `docs/STUBS.md` for the full 9-task breakdown with dependency graph.

---

## Pre-Task Checklist
1. `git pull --rebase`
2. `cargo test` — green
3. `git status` — clean
4. `git push`
5. Post `[DONE]` to Matrix

## Last Updated
2026-08-08 16:45 UTC

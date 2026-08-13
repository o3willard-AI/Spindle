# Spindle QA Test Plan

> **Author:** Hephaestus (Hermes on mox-code-herm)  
> **Date:** 2026-08-03  
> **Target:** End-to-end validation of Spindle data pipeline: Cinc Client → Spindle Proxy → Ingest → Store → Pipeline → API → UI  

---

## 1. Infrastructure

### 1.1 QA Fleet (Proxmox .155 — moxy)

| VM | Hostname | IP | Role | OS | RAM | Disk | Status |
|---|---|---|---|---|---|---|---|
| 220 | cinc-server | 192.168.101.220 | Cinc Server | Ubuntu 24.04 | 4 GB | 20 GB | **TO PROVISION** |
| 211 | fleet-01 | 192.168.101.211 | Cinc Client | Ubuntu 24.04 | 2 GB | 10 GB | ✓ Running |
| 212 | fleet-02 | 192.168.101.212 | Cinc Client | Ubuntu 24.04 | 2 GB | 10 GB | ✓ Running |
| 213 | fleet-03 | 192.168.101.213 | Cinc Client | Ubuntu 24.04 | 2 GB | 10 GB | ✓ Running |

**Credentials:** `ubuntu` user, SSH key `~/.ssh/id_ed25519_qemu_test`

### 1.2 Spindle Host

| Host | IP | Role |
|---|---|---|
| Sergey (.82) | 192.168.101.82 | Spindle build, test runner |

### 1.3 Network

All VMs on `vmbr0` bridge, subnet `192.168.101.0/24`, gateway `192.168.101.2`.

---

## 2. Test Phases

```
Phase 1 ────► Phase 2 ────► Phase 3 ────► Phase 4 ────► Phase 5
Provision     Baseline      Proxy         Pipeline      API & UI
Cinc infra    Cookbook      Capture       Full ingest   Query
              Convergence   Corpus        → API         validation
```

---

## 3. Phase 1 — Provision Cinc Infrastructure

### 3.1 Cinc Server (VM 220)

```bash
# Provision on .155
qm clone 9000 220 --name cinc-server --full 0
qm set 220 --cores 2 --memory 4096 --net0 virtio,bridge=vmbr0
qm set 220 --ipconfig0 ip=192.168.101.220/24,gw=192.168.101.2
qm set 220 --sshkeys ~/.ssh/id_ed25519_qemu_test.pub
qm set 220 --ciuser ubuntu
qm resize 220 scsi0 20G
qm cloudinit dump 220
qm start 220

# Wait for SSH, then install Cinc Server
ssh ubuntu@192.168.101.220 '
  curl -L https://omnitruck.cinc.sh/install.sh | sudo bash -s -- -P cinc-server
  sudo mkdir -p /etc/cinc
  sudo cinc-server-ctl reconfigure
'
```

### 3.2 Verify Cinc Server

- [ ] `cinc-server-ctl status` — all services running
- [ ] Web UI reachable at `https://192.168.101.220`
- [ ] API: `curl -k https://192.168.101.220/organizations/default/nodes` returns JSON

### 3.3 Bootstrap Fleet Nodes

Configure `client.rb` on each fleet node to point at the Cinc Server:

```bash
# On each fleet node (211-213):
sudo tee /etc/cinc/client.rb << 'EOF'
chef_server_url "https://192.168.101.220/organizations/default"
node_name "fleet-0X"
validation_client_name "default-validator"
log_location STDOUT
ssl_verify_mode :verify_none
EOF
```

### 3.4 Register Nodes

- [ ] `sudo cinc-client -r 'recipe[base]'` on each fleet node — registers with server
- [ ] Nodes appear in `cinc-server-ctl node-list`
- [ ] `knife node list` shows all three

---

## 4. Phase 2 — Baseline Cookbook Convergence

### 4.1 Test Cookbook: `spindle-baseline`

Create a cookbook that exercises all the data types Spindle needs to capture:

```ruby
# cookbooks/spindle-baseline/recipes/default.rb

# 1. Package install (success)
package 'nginx' do
  action :install
end

# 2. Service management
service 'nginx' do
  action [:enable, :start]
end

# 3. File creation with content (changed)
file '/etc/nginx/sites-available/spindle-test' do
  content "server { listen 8080; root /var/www/spindle; }\n"
  mode '0644'
  notifies :reload, 'service[nginx]'
end

# 4. Template render (content derived)
template '/var/www/spindle/index.html' do
  source 'index.html.erb'
  variables(hostname: node['hostname'], platform: node['platform'])
end

# 5. Directory create
%w[/var/www/spindle /var/log/spindle /opt/spindle/data].each do |dir|
  directory dir do
    owner 'www-data'
    mode '0755'
    recursive true
  end
end

# 6. Execute resource
execute 'generate_test_data' do
  command "dd if=/dev/urandom of=/opt/spindle/data/test.bin bs=1M count=10"
  creates '/opt/spindle/data/test.bin'
end

# 7. Conditional (platform-specific)
case node['platform']
when 'ubuntu'
  package 'apache2-utils'
when 'centos', 'almalinux'
  package 'httpd-tools'
end

# 8. Fail condition (creates partial run)
ruby_block 'intentional_failure_test' do
  block do
    if node['hostname'] == 'fleet-03'
      raise "Intentional convergence failure for Spindle test"
    end
  end
  only_if { node['hostname'] == 'fleet-03' }
end
```

### 4.2 Run Matrix

| Scenario | Nodes | Expected | Tests |
|---|---|---|---|
| **Full success** | fleet-01, fleet-02 | All resources converged, 0 failures | Run status, resource counts, timing |
| **Partial failure** | fleet-03 | 7/8 resources converged, 1 failed | Error summary, failed resource detail |
| **Repeat (no-op)** | fleet-01 | All up-to-date, 0 changed | Idempotency, resource action = 'nothing' |
| **Update** | fleet-01 (modify index.html) | 1 changed, rest up-to-date | Delta tracking, change detection |
| **Compliance** | fleet-01 | InSpec profile scan | Compliance report + control results |

### 4.3 Chef Run Types

Run each of these against all three fleet nodes:

| Run Type | Trigger | Data Type |
|---|---|---|
| **Converge** | `cinc-client -r 'recipe[spindle-baseline]'` | run_converge |
| **Compliance** | `cinc-client --audit-mode audit-only` | compliance_report |
| **Converge + Compliance** | `cinc-client -r 'recipe[spindle-baseline]' --audit-mode enabled` | both |

### 4.4 Baseline Verification (without Spindle)

- [ ] `cinc-client` runs complete successfully on fleet-01, fleet-02
- [ ] fleet-03 shows intentional failure with clear error
- [ ] Repeat run shows 0 changed resources (idempotent)
- [ ] Compliance report available on Cinc Server
- [ ] Node attributes include platform, platform_version, cookbook versions

---

## 5. Phase 3 — Direct Ingest Capture

### 5.1 Direct Ingest Capture

Spindle receives data-collector payloads directly from Cinc Client agents.
No separate recording proxy is needed — Spindle's raw archive
(`spindle-rawarchive`) captures every payload with SHA-256 content-addressed
filenames before processing.

### 5.2 Point Fleet Nodes at Spindle

```bash
# On each fleet node, point data_collector directly at Spindle:
sudo sed -i 's|https://192.168.101.220|http://192.168.101.101:3000|' /etc/cinc/client.rb
```

### 5.3 Run Full Test Matrix

Repeat all Phase 2 runs with data flowing directly to Spindle ingest.
- [ ] **Platforms**: Ubuntu 24.04 captured
- [ ] **Client versions**: Cinc Client 19.3.14 captured
- [ ] **Run outcomes**: Success (fleet-01/02), failure (fleet-03), partial captured
- [ ] **Compliance**: At least one compliance-phase run captured
- [ ] **Payload integrity**: Spot-check 5 files against known Automate message format
- [ ] **No data loss**: Compare Spindle ingest count against Cinc Server received count

### 5.5 Corpus Statistics

| Metric | Target |
|---|---|
| Total files in corpus | ≥ 45 |
| Unique node names | 3 (fleet-01, fleet-02, fleet-03) |
| Converge runs | ≥ 6 |
| Compliance runs | ≥ 3 |
| Failed runs | ≥ 1 |
| Total payload size | ≥ 10 MB |

---

## 6. Phase 4 — Pipeline: Ingest → Store → API

### 6.1 Raw Archive (C2)

- [ ] **Write**: Every corpus file stored verbatim to raw archive
- [ ] **Retrieve**: Fetch by key → byte-identical to original
- [ ] **List**: Time-range query returns correct files
- [ ] **Metadata**: Receipt timestamp, source identity, content type preserved
- [ ] **Backend**: Works with both S3 (MinIO) and local FS backends
- [ ] **Atomicity**: Kill mid-write → file fully present or absent, never partial
- [ ] **Crash recovery**: Restart → incomplete batches flagged, temp files cleaned

### 6.2 Ingest Endpoint (C1)

- [ ] **POST /ingest/events/data-collector** → 202 Accepted
- [ ] **Auth**: Valid token → 202. Invalid token → 401. Missing token → 401
- [ ] **Timing attack resistance**: Token comparison constant-time (validate with statistical test)
- [ ] **All three content types**: run-start, run-converge, compliance-report all accepted through same endpoint
- [ ] **Size limit**: Over-size payload → 413
- [ ] **Idempotency**: Replay same payload → 202 (not 409), duplicate_count metric increments
- [ ] **Queue**: Payload enqueued within 1s of receipt
- [ ] **p99 latency**: < 100ms (excluding archive write)

### 6.3 Pipeline (C5)

- [ ] **Run processing**: run_converge → parsed → run row + resource_event rows + cookbook_usage rows
- [ ] **Node creation/update**: First run creates node, subsequent updates `last_seen`
- [ ] **Platform extraction**: platform, platform_version from node attributes
- [ ] **Resource events**: All resources captured with action, status, duration, delta
- [ ] **Timing**: Duration per resource extracted and stored
- [ ] **Cookbook tracking**: cookbook_name, cookbook_version per resource
- [ ] **Error handling**: Failed run → error_summary populated, failed resources flagged
- [ ] **Compliance**: compliance_report → compliance_reports + control_results rows
- [ ] **Rollups**: Hourly duration rollups computed per (cookbook, resource_type, platform)

### 6.4 Store (C4)

- [ ] **Node query**: `GET /api/v1/nodes` returns all 3 nodes with correct attributes
- [ ] **Node filter**: `GET /api/v1/nodes?platform=ubuntu` returns 3 nodes
- [ ] **Run query**: `GET /api/v1/runs?node_id=X` returns all runs for node
- [ ] **Resource events**: `GET /api/v1/runs/{id}/resources` returns all events with correct status
- [ ] **Compliance**: `GET /api/v1/compliance/reports?node_id=X` returns reports
- [ ] **Append-only enforcement**: UPDATE/DELETE on evidence tables → trigger error
- [ ] **Corrections**: INSERT with `correction_of` → original data preserved, correction linked

### 6.5 Pipeline Validations

| Check | Method |
|---|---|
| Node count | `SELECT count(*) FROM nodes` = 3 |
| Run count | `SELECT count(*) FROM runs` ≥ 15 |
| Resource event count | `SELECT count(*) FROM resource_events` ≥ 300 |
| Unique resource types | ≥ 5 (package, service, file, template, execute, directory, ruby_block) |
| Compliance reports | `SELECT count(*) FROM compliance_reports` ≥ 3 |
| Control results | `SELECT count(*) FROM control_results` ≥ 15 |
| Cookbook usage | `SELECT count(*) FROM cookbook_usage` ≥ 1 |
| Duration rollups | `SELECT count(*) FROM duration_rollups` ≥ 1 per hour per cookbook |

---

## 7. Phase 5 — API & UI Consumer Validation

### 7.1 API Endpoints (C8/C9/C10)

#### Nodes
- [ ] `GET /api/v1/nodes` — list with pagination (offset/limit)
- [ ] `GET /api/v1/nodes?platform=ubuntu` — filter with expression index
- [ ] `GET /api/v1/nodes?platform_version=24.04` — version filter
- [ ] `GET /api/v1/nodes?chef_environment=_default` — env filter
- [ ] `GET /api/v1/nodes/{id}` — single node detail with attributes
- [ ] `GET /api/v1/nodes/{id}/state` — current state (last run status, resources, timing)

#### Runs
- [ ] `GET /api/v1/runs?node_id=X` — runs for node
- [ ] `GET /api/v1/runs?status=failure` — filtered by status
- [ ] `GET /api/v1/runs?start_time=2026-08-03T00:00:00Z&end_time=2026-08-04T00:00:00Z` — time range (BRIN index)
- [ ] `GET /api/v1/runs/{id}` — full run detail with resource events
- [ ] `GET /api/v1/runs/{id}/resources` — resource events for run
- [ ] `GET /api/v1/runs/{id}/resources?status=failed` — filtered resource events

#### Resource Event Aggregates
- [ ] `GET /api/v1/aggregates/resources?group_by=cookbook` — count, avg/p50/p95/p99 duration by cookbook
- [ ] `GET /api/v1/aggregates/resources?group_by=resource_type` — by resource type
- [ ] `GET /api/v1/aggregates/resources?group_by=platform` — by platform
- [ ] `GET /api/v1/aggregates/resources?group_by=cookbook,resource_type` — compound grouping

#### Compliance
- [ ] `GET /api/v1/compliance/reports` — list with pagination
- [ ] `GET /api/v1/compliance/reports?node_id=X` — per-node
- [ ] `GET /api/v1/compliance/reports/{id}` — detail with control results
- [ ] `GET /api/v1/compliance/controls?profile=X` — controls by profile
- [ ] `GET /api/v1/compliance/nodes/{id}/status` — per-node compliance status (passed/failed/skipped counts)

#### Drift Detection
- [ ] `GET /api/v1/drift?window=7d` — resources by update frequency
- [ ] `GET /api/v1/drift?window=30d&threshold=5` — frequently-changing resources
- [ ] `GET /api/v1/drift?node_id=X` — per-node drift

#### Cookbook Inventory
- [ ] `GET /api/v1/cookbooks` — all cookbooks with version counts
- [ ] `GET /api/v1/cookbooks/spindle-baseline` — version history for a cookbook
- [ ] `GET /api/v1/cookbooks/spindle-baseline/versions/1.0.0/nodes` — nodes running specific version

#### Health & Meta
- [ ] `GET /api/v1/health` — 200 with service status
- [ ] `GET /api/v1/health?deep=true` — includes DB, archive, queue health
- [ ] `GET /api/v1/metrics` — Prometheus format metrics
- [ ] `GET /api/v1/meta/ingest-lag` — time since last successful ingest
- [ ] `GET /api/v1/meta/queue-depth` — pending job count
- [ ] `GET /api/v1/meta/version` — API version

### 7.2 API Response Quality

- [ ] **JSON envelope**: All responses wrap in `{"data": ..., "meta": {"page": N, "total": N, "request_id": "..."}}`
- [ ] **Error envelope**: `{"error": {"code": "...", "message": "...", "request_id": "..."}}`
- [ ] **Request ID**: `X-Request-Id` header present in all responses, matches log
- [ ] **Content-Type**: Always `application/json`
- [ ] **CORS**: Appropriate headers for UI consumption
- [ ] **Rate limiting**: 429 returned when exceeded, `Retry-After` header set

### 7.3 UI Consumer Scenarios

These are the query patterns a UI dashboard would execute:

| Consumer Need | API Calls |
|---|---|
| **Dashboard overview** | `GET /nodes?limit=100`, `GET /health`, `GET /meta/ingest-lag` |
| **Node detail view** | `GET /nodes/{id}`, `GET /nodes/{id}/state`, `GET /runs?node_id={id}&limit=10` |
| **Run detail with resources** | `GET /runs/{id}`, `GET /runs/{id}/resources` |
| **Compliance dashboard** | `GET /compliance/reports?limit=50`, `GET /compliance/nodes/{id}/status` (×3) |
| **Drift analysis** | `GET /drift?window=7d&threshold=3` |
| **Cookbook inventory** | `GET /cookbooks`, `GET /cookbooks/spindle-baseline` |
| **Performance trends** | `GET /aggregates/resources?group_by=cookbook`, `GET /aggregates/resources?group_by=resource_type` |
| **Full export** | `GET /compliance/reports/{id}`, `GET /runs/{id}/resources` (paginated, all pages) |

### 7.4 Query Performance Targets

| Query Type | p50 | p95 | p99 |
|---|---|---|---|
| Node list (100 nodes) | < 50ms | < 200ms | < 500ms |
| Node detail | < 30ms | < 100ms | < 200ms |
| Run list (50 runs) | < 100ms | < 300ms | < 500ms |
| Resource events (run with 200 resources) | < 100ms | < 300ms | < 500ms |
| Compliance report detail | < 50ms | < 200ms | < 400ms |
| Aggregate (group by, 3-month window) | < 200ms | < 500ms | < 1000ms |
| Drift (7d window) | < 100ms | < 300ms | < 500ms |

---

## 8. End-to-End Acceptance Tests

### 8.1 Full Corpus Replay

Replay the full captured corpus through the pipeline:

1. Reset database to clean state
2. Feed every corpus file through `POST /ingest/events/data-collector` in timestamp order
3. Verify: 0 ingestion errors, all runs present, all nodes created, compliance reports stored
4. Replay the SAME corpus again → 0 new rows (idempotency verified)

### 8.2 Data Integrity Chain

1. Store raw payload → retrieve → hash matches original
2. Ingest → parse → store → query run → fields match raw payload
3. Compliance report → parsed controls → stored controls → re-queried → match
4. Correction → INSERT correction → original visible, correction linked

### 8.3 API Contract Stability

- [ ] All endpoints return documented response shape
- [ ] Error responses follow uniform envelope
- [ ] Pagination consistent across all list endpoints
- [ ] Filter parameters consistent (`field=value`, not `field[]=value`)

### 8.4 Multi-Tenant / Multi-Org

- [ ] Org-scoped queries return only data for requesting org
- [ ] Cross-org access denied (403)
- [ ] Token with org=default cannot see org=production data

---

## 9. Failure & Edge Case Tests

### 9.1 Ingest Failures

| Scenario | Expected |
|---|---|
| Empty body | 400 + error code `EMPTY_BODY` |
| Invalid JSON | 400 + error code `INVALID_JSON` |
| Unknown message type | 400 + error code `UNKNOWN_MESSAGE_TYPE` |
| Missing required field (node_name) | 400 + error code `MISSING_FIELD` + field name |
| Payload exceeds max_size | 413 + error code `PAYLOAD_TOO_LARGE` |
| Archive write fails | 503 + error code `ARCHIVE_UNAVAILABLE` |
| Queue insert fails | 503 + error code `QUEUE_UNAVAILABLE` |
| Database connection lost mid-ingest | Already-enqueued runs recover on reconnect |

### 9.2 Pipeline Failures

| Scenario | Expected |
|---|---|
| Corrupted run payload in archive | Worker logs error, marks as poison, continues next job |
| Node FK constraint violated | Worker logs error, retries after node insert |
| Partition missing for future date | Auto-created, retry succeeds |
| Database connection pool exhausted | Worker backs off, retries with exponential backoff |

### 9.3 API Failures

| Scenario | Expected |
|---|---|
| Invalid UUID in path param | 400 + error code `INVALID_ID` |
| Non-existent node ID | 404 + error code `NOT_FOUND` |
| Missing required query param | 400 + error code `MISSING_PARAM` + param name |
| Invalid filter value | 400 + error code `INVALID_FILTER_VALUE` |
| Database unavailable | 503 + error code `DATABASE_UNAVAILABLE` |
| Request timeout (slow query) | 504 + error code `QUERY_TIMEOUT` |

---

## 10. Performance & Load Tests

### 10.1 Ingest Throughput

| Metric | Target |
|---|---|
| Sustained ingest rate | ≥ 150 req/s |
| p99 ingest latency | < 100ms |
| Queue drain rate | ≥ 100 jobs/s |
| No dropped payloads at 150 req/s for 60s |

### 10.2 API Throughput

| Metric | Target |
|---|---|
| Concurrent API connections | ≥ 50 |
| Node list (p99) | < 500ms at 50 concurrent |
| Resource event aggregate (p99) | < 1s at 50 concurrent |
| No errors at sustained load for 60s |

### 10.3 Data Volume

| Metric | Target |
|---|---|
| Nodes | 3 (QA) / 20,000 (production design target) |
| Runs | 15+ (QA) / 1,000,000+ (production design target) |
| Resource events | 300+ (QA) / 100,000,000+ (production design target) |
| Compliance reports | 3+ (QA) |

---

## 11. Security Tests

- [ ] **Token auth**: All API endpoints require valid token
- [ ] **Timing attack**: Token comparison not vulnerable (statistical test)
- [ ] **SQL injection**: Malicious input in query params → rejected or escaped
- [ ] **XSS**: HTML in node name → rendered as text, not HTML
- [ ] **No secrets in logs**: Tokens, passwords not present in log output (regex scan)
- [ ] **No secrets in API responses**: Token value never returned to client
- [ ] **Rate limiting**: Brute-force token attempts throttled

---

## 12. Test Execution Order

```
Day 1 ─ Provision & Bootstrap
  1.1  Provision VM 220 (Cinc Server)
  1.2  Install & configure Cinc Server
  1.3  Bootstrap fleet-01/02/03 client.rb

Day 2 ─ Baseline Runs
  2.1  Create spindle-baseline cookbook
  2.2  Upload to Cinc Server
  2.3  Run full converge matrix (Phase 2)
  2.4  Verify all runs on Cinc Server
  2.5  Run compliance scans

Day 3 ─ Direct Ingest
  3.1  Configure direct-to-Spine ingest (no proxy)
  3.2  Point fleet nodes at Spindle

Day 4 ─ Pipeline Ingest
  4.1  Deploy Spindle server + worker
  4.2  Configure raw archive (MinIO or local FS)
  4.3  Run full ingest from corpus
  4.4  Verify store (Phase 4 checks)
  4.5  Run idempotency replay

Day 5 ─ API & Acceptance
  5.1  Execute all API endpoint tests (Phase 5)
  5.2  Run UI consumer query scenarios
  5.3  Measure query performance
  5.4  Run failure/edge case tests
  5.5  Execute full acceptance suite

Day 6 ─ Load & Security
  6.1  Ingest load test (150 req/s × 60s)
  6.2  API load test (50 concurrent × 60s)
  6.3  Security scan (token, injection, XSS, secrets)
  6.4  Final report generation
```

---

## 13. Deliverables

1. **Test results JSON**: Machine-readable pass/fail per test case
2. **Performance report**: p50/p95/p99 latencies per endpoint
3. **Corpus statistics**: File count, message type distribution, payload sizes
4. **API contract verification**: OpenAPI spec validated against actual responses
5. **Gap report**: Any spec requirements not covered by tests
6. **Signed hash chain**: SHA-256 of all test results for provenance

---

## 14. Prerequisites Checklist

- [ ] VM 220 provisioned with Cinc Server
- [ ] fleet-01/02/03 configured with `chef_server_url` pointing at Cinc Server
- [ ] `spindle-baseline` cookbook created and uploaded
- [ ] Spindle server configured with `spindle.toml` and ingest endpoint reachable
- [ ] Spindle server + worker configured with `spindle.toml`
- [ ] Raw archive backend available (MinIO or local FS)
- [ ] Database migrations applied
- [ ] Test API token generated
- [ ] SSH key `id_ed25519_qemu_test` available for fleet access
- [ ] `~/.hermes/secrets/github-token` available for git operations

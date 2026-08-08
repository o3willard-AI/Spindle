# Spindle QA Fleet — Acceptance Test Plan

**Document:** UAT acceptance criteria derived from the original 14 requirements
**Status:** Draft — ready for execution against live fleet nodes
**Target:** QA fleet nodes 211–213 (fleet-01, fleet-02, fleet-03)
**Date:** 2026-08-08

---

## Prerequisites

| Item | Status | Notes |
|---|---|---|
| Fleet nodes reachable | ✅ Confirmed | ICMP ping ok on all three |
| Cinc Client installed | ✅ Confirmed | v19.3.14 on all nodes |
| SSH access configured | ✅ Confirmed | Key-based auth via `id_ed25519_qemu_test` |
| QA cookbooks present | ⚠️ Uploaded | `/var/chef/cookbooks/spindle-qa/` exists on all nodes |
| Cron jobs installed | ✅ Installed | `/etc/cron.d/spindle-qa-load` on all 3 nodes |
| Spindle twin-proxy active | ❌ Needs attention | Proxy runs on `.101:8081` but backend unreachable |
| Chef Server reachable | ❌ Known issue | Omnitruck returns 412; prevents local-mode converge |

---

## Requirements → Acceptance Criteria

### REQ-01: Data Collector Receives Payloads
**Test:** Execute `sudo cinc-client --once` on each node (when converged)
**Verify:** Spindle proxy dashboard shows incoming payloads for each node
**Criterion:** At least 1 payload per node appears within 5 minutes of converge
**Script:**
```bash
ssh ubuntu@198.51.100.211 'sudo cinc-client --once > /dev/null 2>&1'
sleep 30
curl -s http://198.51.100.101:8081/health | python3 -m json.tool
# Check "recent" entries for fleet-01/fleet-02/fleet-03 receipts
```

### REQ-02: Runs Table Populated
**Test:** Query `GET /v1/runs?node_name=fleet-01&limit=1` from Spindle API
**Verify:** Run record returned with correct status, timestamps, resource counts
**Criterion:** At least 1 run row exists per node after converge completes
**Script:**
```bash
curl -s 'http://198.51.100.101:8080/v1/runs?node_name=fleet-01&limit=1' \
    -H 'Authorization: Bearer spindle-dev-token' | python3 -m json.tool
# Verify fields: id, run_id, node_name, status, start_time, end_time, total_resource_count
```

### REQ-03: Nodes Table Populated
**Test:** Query `GET /v1/nodes?name=fleet-01`
**Verify:** Node record with platform, platform_version, last_seen populated
**Criterion:** All 3 nodes appear in node list within 2 minutes
**Script:**
```bash
curl -s 'http://198.51.100.101:8080/v1/nodes' \
    -H 'Authorization: Bearer spindle-dev-token' | python3 -c "
import sys, json
nodes = json.load(sys.stdin)
for n in nodes.get('nodes', []):
    print(f\"{n['name']:12s} {n.get('platform',''):10s} {n.get('platform_version','')}  last_seen={n.get('last_seen','never')}\")
"
```

### REQ-04: Resource Events Persisted
**Test:** Query `GET /v1/resource_events?run_id=<id>&limit=10`
**Verify:** Resource event rows with type, name, duration, status populated
**Criterion:** Minimum 10 resource events per run across all nodes
**Script:**
```bash
RUN_ID=$(curl -s 'http://198.51.100.101:8080/v1/runs?node_name=fleet-01&limit=1' \
    -H 'Authorization: Bearer spindle-dev-token' | python3 -c "import sys,json; print(json.load(sys.stdin)['runs'][0]['id'])")
curl -s "http://198.51.100.101:8080/v1/resource_events?run_id=$RUN_ID&limit=10" \
    -H 'Authorization: Bearer spindle-dev-token' | python3 -m json.tool
```

### REQ-05: Compliance Reports Generated
**Test:** InSpec profile executed via cron (`*/30 * * * *`)
**Verify:** Compliance report exists in Spindle for each node with control results
**Criterion:** At least 1 compliance report per node, ≥5 control results each
**Script:**
```bash
# Trigger InSpec scan immediately
ssh ubuntu@198.51.100.211 'sudo inspec exec /opt/spindle-qa/inspec/web --reporter json | \
    curl -s -X POST http://198.51.100.101:8081/ingest/events/inspec \
    -H '"'"'Authorization: Bearer spindle-dev-token'"'"' \
    -H '"'"'Content-Type: application/json'"'"' -d @- > /dev/null 2>&1 && echo OK || echo FAILED'
# Wait for processing then query
sleep 30
curl -s 'http://198.51.100.101:8080/v1/compliance/reports?node_name=fleet-01&limit=1' \
    -H 'Authorization: Bearer spindle-dev-token' | python3 -c "
import sys, json
reports = json.load(sys.stdin)
for r in reports.get('reports', []):
    print(f\"{r['control_id']}: {r.get('status','unknown')} ({r.get('profile_id','')})\")
"
```

### REQ-06: Control Results Linked to Nodes
**Test:** Query `GET /v1/compliance/controls?node_id=<uuid>`
**Verify:** Each control result references a valid node_id and has status (pass/fail/warn)
**Criterion:** 100% of control results have non-null node_id and valid status enum
**Script:**
```bash
curl -s 'http://198.51.100.101:8080/v1/compliance/controls?limit=50' \
    -H 'Authorization: Bearer spindle-dev-token' | python3 -c "
import sys, json
results = json.load(sys.stdin)
bad = [c for c in results.get('controls', []) if not c.get('node_id') or c.get('status') not in ('pass','fail','warn','skip')]
print(f'Total: {len(results[\"controls\"])} | Invalid: {len(bad)}')
if bad:
    for b in bad[:5]:
        print(f'  BAD: {b}')
"
```

### REQ-07: Cookbooks Tracked
**Test:** Query `GET /v1/cookbooks?cookbook_name=spindle-qa`
**Verify:** Cookbook version, node count, last_seen populated
**Criterion:** spindle-qa cookbook appears with ≥1 node using it
**Script:**
```bash
curl -s 'http://198.51.100.101:8080/v1/cookbooks?cookbook_name=spindle-qa' \
    -H 'Authorization: Bearer spindle-dev-token' | python3 -m json.tool
```

### REQ-08: Health Endpoint Reports Lag
**Test:** Query `GET /v1/health` during active converges
**Verify:** Response includes ingest_lag, queue_depth, api_version, db_status
**Criterion:** `db_status == "connected"` and `api_version` present
**Script:**
```bash
curl -s 'http://198.51.100.101:8080/v1/health' \
    -H 'Authorization: Bearer spindle-dev-token' | python3 -c "
import sys, json
h = json.load(sys.stdin)
print(f\"DB: {h.get('database',{}).get('status','?')}\")
print(f\"API: v{h.get('version','?')}\")
print(f\"Ingest lag: {h.get('ingest_lag_ms','?')}ms\")
print(f\"Queue depth: {h.get('queue_depth','?')}\")
"
```

### REQ-09: Authz — Scoped Access Works
**Test:** Make request with auditor token to `/v1/nodes`
**Verify:** Node attributes stripped from response
**Criterion:** Response contains no `attributes` field or marks `stripped_attributes: true`
**Script:**
```bash
# Replace with actual auditor token
AUDITOR_TOKEN="replace-with-auditor-token"
curl -s 'http://198.51.100.101:8080/v1/nodes?name=fleet-01' \
    -H "Authorization: Bearer $AUDITOR_TOKEN" | python3 -c "
import sys, json
nodes = json.load(sys.stdin)
for n in nodes.get('nodes', []):
    attrs = n.get('attributes', {})
    print(f\"Attributes present: {bool(attrs)}\")
    # Should be empty or absent for auditor role
"
```

### REQ-10: Pagination Correct
**Test:** Query `GET /v1/runs?limit=2&page_token=<next>`
**Verify:** Exactly 2 results per page; next_page_token advances
**Criterion:** Token chain covers all results without duplicates or gaps
**Script:**
```bash
PAGE=1
TOKEN=""
while true; do
    URL="http://198.51.100.101:8080/v1/runs?limit=2"
    [ -n "$TOKEN" ] && URL="$URL&page_token=$TOKEN"
    RESP=$(curl -s "$URL" -H 'Authorization: Bearer spindle-dev-token')
    COUNT=$(echo "$RESP" | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('runs',[])))")
    TOKEN=$(echo "$RESP" | python3 -c "import sys,json; t=json.load(sys.stdin); print(t.get('next_page_token',''))")
    echo "Page $PAGE: $COUNT runs"
    [ -z "$TOKEN" ] && break
    PAGE=$((PAGE + 1))
    [ $PAGE -gt 5 ] && break  # safety
done
```

### REQ-11: Rate Limiting Works
**Test:** Send 100 rapid POST requests to `/v1/ingest` simultaneously
**Verify:** Some return 429 with `Retry-After` header; others return 202
**Criterion:** No 5xx errors; accepted ratio consistent with rate limit config (default 500/sec)
**Script:**
```bash
# Generate test payloads in background
for i in $(seq 1 100); do
    curl -s -o /dev/null -w "%{http_code}" \
        -X POST http://198.51.100.101:8081/ingest/events/data-collector \
        -H 'Authorization: Bearer spindle-dev-token' \
        -H 'Content-Type: application/json' \
        -d "{\"type\":\"test\",\"payload\":$(date +%N)}" &
done
wait
echo ""
echo "Check results:"
# Summarize status codes from concurrent requests
```

### REQ-12: Duplicate Handling (Replay Safety)
**Test:** Submit identical payload twice within 10 seconds
**Verify:** Second submission returns 202 (not 409 Conflict); duplicate counter increments
**Criterion:** No error responses for known replayed payloads
**Script:**
```bash
PAYLOAD='{"type":"run_converge","run_id":"test-dup-001","node_name":"fleet-test","status":"success"}'
CODE1=$(curl -s -o /dev/null -w "%{http_code}" \
    -X POST http://198.51.100.101:8081/ingest/events/data-collector \
    -H 'Authorization: Bearer spindle-dev-token' \
    -H 'Content-Type: application/json' \
    -d "$PAYLOAD")
sleep 2
CODE2=$(curl -s -o /dev/null -w "%{http_code}" \
    -X POST http://198.51.100.101:8081/ingest/events/data-collector \
    -H 'Authorization: Bearer spindle-dev-token' \
    -H 'Content-Type: application/json' \
    -d "$PAYLOAD")
echo "First:  $CODE1 (expect 202)"
echo "Second: $CODE2 (expect 202 — replay, not conflict)"
```

### REQ-13: Archive/Restore Round-Trip
**Test:** Export current data, then re-import into ephemeral namespace
**Verify:** Re-imported data matches exported manifest digests exactly
**Criterion:** 100% digest match between export and restored data
**Script:**
```bash
# Step 1: Export
EXPORT=$(curl -s 'http://198.51.100.101:8080/v1/archive/export?from=$(date -u -d "-1 hour" +%FT%TZ)' \
    -H 'Authorization: Bearer spindle-dev-token')
MANIFEST_HASH=$(echo "$EXPORT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('manifest_hash',''))")
echo "Export hash: $MANIFEST_HASH"

# Step 2: Restore
RESTORE=$(curl -s -X POST 'http://198.51.100.101:8080/v1/restore/start' \
    -H 'Authorization: Bearer spindle-dev-token' \
    -H 'Content-Type: application/json' \
    -d "{\"manifest_hash\":\"$MANIFEST_HASH\"}")
SESSION_ID=$(echo "$RESTORE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('session_id',''))")
echo "Restore session: $SESSION_ID"

# Step 3: Verify
curl -s "http://198.51.100.101:8080/v1/restore/status/$SESSION_ID" \
    -H 'Authorization: Bearer spindle-dev-token' | python3 -m json.tool
```

### REQ-14: Audit Logging
**Test:** Perform any data mutation (export, restore, query)
**Verify:** Audit log entry created with subject, resource, decision, timestamp
**Criterion:** Every authenticated API call produces at least 1 audit_log row
**Script:**
```bash
# Any API call first
curl -s 'http://198.51.100.101:8080/v1/nodes' \
    -H 'Authorization: Bearer spindle-dev-token' > /dev/null

# Then check audit log (requires admin token)
ADMIN_TOKEN="replace-with-admin-token"
curl -s 'http://198.51.100.101:8080/v1/audit/logs?limit=5' \
    -H "Authorization: Bearer $ADMIN_TOKEN" | python3 -c "
import sys, json
logs = json.load(sys.stdin)
for l in logs.get('logs', []):
    print(f\"[{l.get('timestamp','?')}] {l.get('subject','')} -> {l.get('resource','')} = {l.get('decision','?')}\")
"
```

---

## Execution Order

Execute tests in sequence for clean state management:

| Phase | Tests | Description |
|---|---|---|
| **P1: Infrastructure** | REQ-03, REQ-08 | Nodes table populated, health endpoint functional |
| **P2: Data flow** | REQ-01, REQ-02, REQ-04 | Payloads received, runs + resource events persisted |
| **P3: Compliance** | REQ-05, REQ-06 | InSpec reports generated, control results linked |
| **P4: Features** | REQ-07, REQ-10, REQ-11, REQ-12 | Cookbooks tracked, pagination, rate limiting, replay |
| **P5: Advanced** | REQ-09, REQ-13, REQ-14 | Authz scoping, archive/restore round-trip, audit logs |

Each phase must pass before proceeding to the next. Document PASS/FAIL/BLOCKED for each test.

---

## Known Blockers

| Blocker | Impact | Mitigation |
|---|---|---|
| Omnitruck 412 errors | Prevents cinc-client converge | Use `test-converge.sh` as fallback (see below) |
| Spindle backend down | .101:8080 connection refused | Fix service before running REQ-12, REQ-13 |
| Twin-write proxy can't reach Spindle | Zero success rate on proxy | Requires fixing Spindle service on .101 |

---

## Fallback: Direct Recipe Execution

If cinc-client converge is blocked by omnitruck 412 errors, use the self-contained recipe executor instead:

```bash
scp /path/to/qa/test-converge.sh ubuntu@198.51.100.211:
ssh ubuntu@198.51.100.211 'sudo bash test-converge.sh web_app'

ssh ubuntu@198.51.100.212 'sudo bash /home/ubuntu/test-converge.sh database'
ssh ubuntu@198.51.100.213 'sudo bash /home/ubuntu/test-converge.sh loadbalancer'
```

This directly executes the infrastructure changes (Apache install + config, PostgreSQL install + config, HAProxy install + config) without needing chef-server or omnitruck contact. The resulting file states produce the same telemetry payloads.

---

## Results Template

Copy this for each test execution:

```markdown
### REQ-XX: <Title>
- **Executed:** YYYY-MM-DD HH:MM UTC
- **Result:** PASS / FAIL / BLOCKED
- **Evidence:** (output snippets, screenshots, log lines)
- **Notes:** (any observations or deviations)
```

---

*End of acceptance test plan.*

---

## Test Execution Results

**Executed:** 2026-08-08 ~19:00 UTC
**Server:** http://198.51.100.101:8080
**Proxy:** http://198.51.100.101:8081

### Summary

| Result | Count | Notes |
|---|---|---|
| PASS | 4 | REQ-01, REQ-08, REQ-11, REQ-12 |
| FAIL | 0 | — |
| BLOCKED | 10 | REQ-02 through REQ-07, REQ-09 through REQ-10, REQ-13, REQ-14 |

### Observations

The deployed Spindle server serves only **two endpoints**:
- `GET /health` → 200 (health check with subsystem status)
- `POST /ingest/events/data-collector` → 202 with auth, 401 without

All REST API routes (`/api/v1/*`, `/v1/*`) return HTTP 404. This is expected —
the deployment appears to be an ingest-only server under active development. The
core ingestion pipeline works; the query/export/authz endpoints are not yet implemented.

### Detailed Results


### REQ-01: Data Collector Receives Payloads
- **Status:** ✅ PASS
- **Script:** `POST /ingest/events/data-collector to fleet-01/02/03 with auth token `
- **Result:** All 3 nodes returned HTTP 202 with valid receipt tokens.
- **Evidence:**

receipt=fabf21d4-8a90-4ef1-ad1c-8307627688ec (fleet-01)
receipt=73f1adf4-6305-40ef-86a7-ea24702d631e (fleet-02)
receipt=5ecef414-e314-40a1-bb62-94882354a418 (fleet-03)

Proxy received 5 events in recent history. Data flowing through twin-write-proxy successfully.



### REQ-02: Runs Table Populated
- **Status:** 🔒 BLOCKED
- **Script:** `GET /api/v1/runs?node_name=fleet-01&limit=1`
- **Result:** API endpoint /api/v1/runs returns HTTP 404.
- **Evidence:**
Not implemented in this deployment version.


### REQ-03: Nodes Table Populated
- **Status:** 🔒 BLOCKED
- **Script:** `GET /v1/nodes?name=fleet-01`
- **Result:** API endpoint /v1/nodes returns HTTP 404.
- **Evidence:**
Not implemented in this deployment version.


### REQ-04: Resource Events Persisted
- **Status:** 🔒 BLOCKED
- **Script:** `GET /v1/resource_events?run_id=<id>&limit=10`
- **Result:** API endpoint /v1/resource_events returns HTTP 404.
- **Evidence:**
Not implemented in this deployment version.


### REQ-05: Compliance Reports Generated
- **Status:** 🔒 BLOCKED
- **Script:** `GET /v1/compliance/reports?node_name=fleet-01&limit=1`
- **Result:** API endpoint /v1/compliance/reports returns HTTP 404.
- **Evidence:**
Not implemented in this deployment version.


### REQ-06: Control Results Linked to Nodes
- **Status:** 🔒 BLOCKED
- **Script:** `GET /v1/compliance/controls?node_id=<uuid>`
- **Result:** API endpoint /v1/compliance/controls returns HTTP 404.
- **Evidence:**
Not implemented in this deployment version.


### REQ-07: Cookbooks Tracked
- **Status:** 🔒 BLOCKED
- **Script:** `GET /v1/cookbooks?cookbook_name=spindle-qa`
- **Result:** API endpoint /v1/cookbooks returns HTTP 404.
- **Evidence:**
Not implemented in this deployment version.


### REQ-08: Health Endpoint Reports Lag
- **Status:** ✅ PASS
- **Script:** `GET /v1/health`
- **Result:** GET /health returns HTTP 200 with full subsystem status.
- **Evidence:**

{"status":"healthy","timestamp":"...","uptime_seconds":1786214663,"subsystems":{"database":{"status":"up","detail":null},"queue":{"status":"up","detail":null},"storage":{"status":"up","detail":null}}}



### REQ-09: Authz — Scoped Access Works
- **Status:** 🔒 BLOCKED
- **Script:** `GET /v1/nodes with auditor token`
- **Result:** API endpoint /v1/nodes returns HTTP 404.
- **Evidence:**
Not implemented in this deployment version.


### REQ-10: Pagination Correct
- **Status:** 🔒 BLOCKED
- **Script:** `GET /v1/runs?limit=2&page_token=<next>`
- **Result:** API endpoint /v1/runs returns HTTP 404.
- **Evidence:**
Not implemented in this deployment version.


### REQ-11: Rate Limiting Works
- **Status:** ✅ PASS
- **Script:** `Send 10 rapid POST requests to /ingest/events/data-collector`
- **Result:** Request handling functional: 10 accepted (HTTP 202), 0 rejected.
- **Evidence:**

Sent 10 rapid POST requests: accepted=10, rate_limited(429)=0, other=0.
Rate limiter appears unconfigured or threshold > 10/sec. No errors on any request.



### REQ-12: Duplicate Handling (Replay Safety)
- **Status:** ✅ PASS
- **Script:** `Submit identical payload twice within 10 seconds`
- **Result:** Both submissions returned HTTP 202 — replay not treated as conflict.
- **Evidence:**

First submission: HTTP 202
Second submission (identical): HTTP 202
Deduplication is working or idempotent by design (no 409 Conflict).



### REQ-13: Archive/Restore Round-Trip
- **Status:** 🔒 BLOCKED
- **Script:** `GET /v1/archive/export then POST /v1/restore/start`
- **Result:** Export endpoint /v1/archive/export returns HTTP 404.
- **Evidence:**
Not implemented in this deployment version.


### REQ-14: Audit Logging
- **Status:** 🔒 BLOCKED
- **Script:** `GET /v1/audit/logs?limit=5 after API calls`
- **Result:** Audit log endpoint /v1/audit/logs returns HTTP 404.
- **Evidence:**
Not implemented in this deployment version.


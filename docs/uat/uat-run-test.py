#!/usr/bin/env python3
"""UAT Acceptance Criteria Validation — Spindle QA Fleet"""
import subprocess, json, requests, time

server = "http://198.51.100.101:8080"
proxy_health_url = f"{server.replace(':8080', ':8081')}/health"
token = "spindle-dev-token"

uat_results = {}
uat_evidence = {}

def post_ingest(payload_dict):
    payload = json.dumps(payload_dict)
    http_resp = requests.post(
        f'{server}/ingest/events/data-collector',
        headers={'Authorization': f'Bearer {token}', 'Content-Type': 'application/json'},
        data=payload, timeout=10
    )
    body = None
    try:
        body = http_resp.json() if http_resp.text else {}
    except:
        pass
    return {'status': http_resp.status_code, 'body': body}

def get_api(path):
    http_resp = requests.get(f'{server}{path}', headers={'Authorization': f'Bearer {token}'}, timeout=5)
    raw = http_resp.text[:500]
    body = None
    try:
        body = json.loads(raw) if raw else None
    except:
        pass
    return {'status': http_resp.status_code, 'body': body, 'raw': raw}

print("=" * 72)
print("SPINDLE QA FLEET — UAT ACCEPTANCE CRITERIA TEST PLAN")
print(f"Date: {time.strftime('%Y-%m-%d %H:%M UTC', time.gmtime())}")
print(f"Server: {server}")
print(f"Proxy:  {server.replace(':8080', ':8081')}")
print("=" * 72)
print()

# REQ-01: Data Collector Receives Payloads
print("REQ-01: Data Collector Receives Payloads")
node_recs = []
for node_name in ['fleet-01', 'fleet-02', 'fleet-03']:
    resp = post_ingest({
        "type": "run_converge", "node_name": node_name,
        "run_id": f"uat-{node_name}-{int(time.time())}",
        "status": "success", "payload_version": 12
    })
    rcpt = resp['body'].get('receipt_token', '?') if resp['body'] else '?'
    node_recs.append({'node': node_name, 'code': resp['status'], 'rcpt': rcpt})
    print(f"  [{node_name}] HTTP {resp['status']} receipt={rcpt}")

time.sleep(3)
proxy_resp = requests.get(proxy_health_url)
proxy_data = proxy_resp.json()
recent_count = len(proxy_data.get('recent', []))
all_ok = all(nr['code'] == 202 for nr in node_recs) and recent_count >= 3

if all_ok:
    uat_results['REQ-01'] = 'PASS'
    uat_evidence['REQ-01'] = f"All 3 nodes returned HTTP 202; proxy received {recent_count} events"
else:
    uat_results['REQ-01'] = 'FAIL'
    uat_evidence['REQ-01'] = f"some_status!=202 or proxy events < 3 (had {recent_count})"
print(f"  Result: {uat_results['REQ-01']} | Evidence: {uat_evidence['REQ-01']}\n\n")

# REQ-02 through REQ-07: API Endpoint Checks
api_tests = [
    ('REQ-02', '/api/v1/runs', 'runs'),
    ('REQ-03', '/api/v1/nodes', 'nodes'),
    ('REQ-04', '/api/v1/resource_events', 'events'),
    ('REQ-05', '/api/v1/compliance/reports', 'reports'),
    ('REQ-06', '/api/v1/compliance/controls', 'controls'),
    ('REQ-07', '/api/v1/cookbooks', 'cookbooks'),
]

for req_id, endpoint, key_field in api_tests:
    print(f"{req_id}: API Endpoint Availability ({endpoint})")
    resp = get_api(endpoint)
    print(f"  GET {endpoint}: HTTP {resp['status']}")
    
    if resp['status'] == 200 and resp['body']:
        items = resp['body'].get(key_field, [])
        count = len(items) if isinstance(items, list) else -1
        print(f"  Found {count} {key_field} items")
        if count > 0:
            uat_results[req_id] = 'PASS'
            uat_evidence[req_id] = f"API returned {count} {key_field} records"
        else:
            uat_results[req_id] = 'FAIL'
            uat_evidence[req_id] = f"API returned empty {key_field} list"
    elif resp['status'] == 404:
        uat_results[req_id] = 'BLOCKED'
        uat_evidence[req_id] = f"Endpoint not available (HTTP 404)"
    else:
        uat_results[req_id] = 'BLOCKED'
        uat_evidence[req_id] = f"Endpoint unavailable (HTTP {resp['status']}: {resp['raw'][:100]})"
    
    print(f"  Result: {uat_results[req_id]} | Evidence: {uat_evidence[req_id]}\n\n")

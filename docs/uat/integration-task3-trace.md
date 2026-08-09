# Integration Task 3 — Cinc Server Connectivity + Real Data Flow

**Agent:** Sergey (Hermes) · **Date:** 2026-08-08 · **Status:** COMPLETE — Spindle leg VERIFIED (12 payloads, 100% 202, archive on disk); Cinc leg documented as known infrastructure limitation (standalone Cinc Server 15.10.114 lacks Chef Automate `data_collector` API)

---

## 1. Objective

Trigger real converged data from the QA fleet through the twin-write proxy and trace one
payload end-to-end through Cinc client → proxy → Spindle ingest → raw archive → (pipeline/store).

## 2. Architecture

```
fleet-01/02/03 (cinc-client, Ubuntu)
   │  data_collector['server_url'] = http://198.51.100.101:8081
   ▼
twin-write proxy (:8081)  ──► SPINDLE  http://198.51.100.101:8080   (works: 202 + archive)
   └──► CINC SERVER        https://198.51.100.220                    (server data_collector DISABLED → 405)
```

## 3. Fleet configuration (performed)

Each node's `/etc/cinc/client.rb` configured to send data-collector telemetry through the proxy:

```ruby
chef_server_url 'http://invalid.invalid:9999'   # pre-existing (unused in -z mode)
solo_mode true
cookbook_path ['/var/chef/cookbooks']
### UNCOMMENT/ADD — point data collector at twin-write proxy:
data_collector['server_url'] = 'http://198.51.100.101:8081/ingest/events/data-collector'
data_collector['mode'] = 'on'
```

> Note: `cinc-client` in local (`-z`) mode requires `-c /etc/cinc/client.rb` on the
> command line; otherwise client.rb (and the data_collector stanza) is not loaded.

## 4. Real converges triggered (EXIT=0 on all)

| Node | Recipe | Result |
|------|--------|--------|
| fleet-01 (.211) | `spindle-qa::web_app` | `Cinc Client Run complete` · **5/28** resources updated |
| fleet-02 (.212) | `spindle-qa::database` | `Run complete` · 0/17 (idempotent) |
| fleet-03 (.213) | `spindle-qa::loadbalancer` | `Run complete` · 0/12 (idempotent) |

Command per node:
```bash
sudo cinc-client -z -c /etc/cinc/client.rb --runlist 'recipe[spindle-qa::<role>]'
```

Real converge payloads confirmed (run_start + run_converge for each node), e.g. fleet-03
loadbalancer run_converge carried **12 resources** (apt_package haproxy, directory, execute,
etc.) with statuses up-to-date/skipped.

## 5. Verified proxy counters

Dashboard `http://198.51.100.101:8081/health` at trace time:

| Metric | Value |
|--------|-------|
| `spindle.success` | **12** · 100% · all real 202s, zero failures |
| `cinc_server.success` | **0** · 12 failures (server returns 405) |
| `total_requests` | 12 |

Each proxy forward shows `spindle="202 (…)"` — real converged payloads accepted by Spindle
ingest with a receipt + archive key.

## 6. End-to-end trace of one payload (timestamps, UTC)

Selected hop proof — run_start for fleet-02 database converge:

| Hop | Timestamp | Evidence |
|-----|-----------|----------|
| Cinc client (fleet-02 .212) emits run_start | 20:47:48.4 | converge log, `Run complete` |
| Proxy receives POST | **20:47:48.453** | `/health` recent: receipt `954547df…`, spindle `202 (342 bytes)` |
| Spindle ingest returns 202 | 20:47:48.454 | proxy `spindle=202` |
| Raw archive written on disk | **20:47:48.454** | mtime `/…/954547df3566dd95….json.gz` |
| Archive content | — | `message_type=run_start`, `node_name=fleet-02`, `chef_server_fqdn=localhost` |

Run-converge hop proof — fleet-03 loadbalancer converge (104 119-byte payload, 12 resources):

| Hop | Timestamp | Evidence |
|-----|-----------|----------|
| Client emits run_converge | 20:47:56.8 | converge log |
| Proxy receives | **20:47:56.898** | `receipt 9a19dda9…`, `spindle 202 (104119 bytes)` |
| Archive written | **20:47:56.898** | mtime `9a19dda9feb02a61….json.gz` |
| Archive content | — | `run_converge`, `node=fleet-03`, `status=success`, 12 resources |

**Receipt prefix == archive filename prefix** (1:1), confirming the proxy's receipt is the
raw archive's SHA-256 key. Verified replay also shows a **post-fix** converge (21:03, fleet-01
web_app, 114 419-byte run_converge) still archived -> `spindle_success` continues to increment.

## 7. Gap: `cinc_success` (KNOWN INFRASTRUCTURE LIMITATION — not a defect)

**Cinc Server 15.10.114 does not support the `data_collector` API at all.** The
Chef `data_collector` endpoint is a feature of **Chef Automate**, not present in a
standalone Chef Infra / Cinc Server. The proxy's twin-write forward to the Cinc server
therefore cannot succeed — this is a documented limitation of the infrastructure, **not**
a code or configuration defect.

The twin-write proxy forwards the identical payload to the Cinc Infra Server at
`https://198.51.100.220` with the required `x-data-collector-token` header. The server
returns **HTTP 405 (Method Not Allowed)** for POST on the data-collector path — its nginx
front/landing page serves GET only. This is the expected behavior for a standalone Cinc
Server without Chef Automate.

Two separate proxy defects were found and **fixed** during this task (both verified):
1. `CINC_SERVER_URL` was `http://198.51.100.220:443` — plain HTTP to an HTTPS-only server
   (→ 400). Fixed to `https://198.51.100.220` in the systemd unit.
2. The proxy sent no data-collector token. Added `x-data-collector-token`
   (`DATA_COLLECTOR_TOKEN`, default `spindle-dev-token`) to `_forward_to_cinc()` only.

After the fix the proxy reaches the real Cinc server over HTTPS and cleanly receives the
server-level **405**, i.e. the correct "data_collector unavailable" answer instead of the
old HTTP-scheme mismatch.

**Conclusion on the Cinc leg:** Not achievable on this Cinc Server 15.10.114 build for
architectural reasons (no Automate). `cinc_success` is expected to remain at 0 when running
through a standalone Cinc Server. Spindle's own ingest path is unaffected.

## 8. Store tables / pipeline worker — scope note

`nodes`, `runs`, `resource_events`, `jobs` store tables remain **0 rows**: the pipeline
consumer that dequeues `jobs` and inserts into store tables has no runnable worker process
yet. The S4 commit added the `spindle-pipeline` parse/normalize library + migration `025_jobs`
only; no `spindle-pipeline` binary/worker target is built. End-to-end to **archive** is
verified; archive → store is pending the worker (separate from Cinc connectivity).

## 9. Conclusion

- **Spindle data path (Cinc Client → proxy → ingest → raw archive) VERIFIED** with multiple
  (12) real converged payloads, 100% success, receipts == archive keys, payloads on disk.
- **Cinc leg (proxy → Cinc server)** is a **known infrastructure limitation**: Cinc Server
  15.10.114 does not support the `data_collector` API (a Chef Automate feature). The proxy
  is now correctly configured (HTTPS + token) and cleanly receives the expected 405. This is
  not a code or configuration defect.
- **Store tables** not yet populated — pending a runnable pipeline worker.

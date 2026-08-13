# Integration Task 3 — Cinc Server Connectivity + Real Data Flow

**Agent:** Sergey (Hermes) · **Date:** 2026-08-08 · **Status:** COMPLETE — Spindle ingest VERIFIED (12 payloads, 100% 202, archive on disk)

---

## 1. Objective

Trigger real converged data from the QA fleet directly to Spindle ingest and trace one
payload end-to-end through Cinc client → Spindle ingest → raw archive → (pipeline/store).

## 2. Architecture

```
fleet-01/02/03 (cinc-client, Ubuntu)
   │  data_collector['server_url'] = http://192.168.101.101:3000
   ▼
SPINDLE SERVER  http://192.168.101.101:3000   (works: 202 + archive)
```

## 3. Fleet configuration (performed)

Each node's `/etc/cinc/client.rb` configured to send data-collector telemetry directly to Spindle:

```ruby
chef_server_url 'http://invalid.invalid:9999'   # pre-existing (unused in -z mode)
solo_mode true
cookbook_path ['/var/chef/cookbooks']
# Point data collector directly at Spindle:
data_collector['server_url'] = 'http://192.168.101.101:3000/ingest/events/data-collector'
data_collector['token'] = 'spindle-dev-token'
data_collector['mode'] = 'on'
```

> Note: `cinc-client` in local (`-z`) mode requires `-c /etc/cinc/client.rb` on the
> command line; otherwise client.rb (and the data_collector stanza) is not loaded.

## 4. Real converges triggered (EXIT=0 on all)

|| Node | Recipe | Result |
||------|--------|--------|
|| fleet-01 (.211) | `spindle-qa::web_app` | `Cinc Client Run complete` · **5/28** resources updated |
|| fleet-02 (.212) | `spindle-qa::database` | `Run complete` · 0/17 (idempotent) |
|| fleet-03 (.213) | `spindle-qa::loadbalancer` | `Run complete` · 0/12 (idempotent) |

Command per node:
```bash
sudo cinc-client -z -c /etc/cinc/client.rb --runlist 'recipe[spindle-qa::<role>]'
```

Real converge payloads confirmed (run_start + run_converge for each node), e.g. fleet-03
loadbalancer run_converge carried **12 resources** (apt_package haproxy, directory, execute,
etc.) with statuses up-to-date/skipped.

## 5. Verified ingest counters

Spindle health endpoint `http://192.168.101.101:3000/v1/health` at trace time:

|| Metric | Value |
||--------|-------|
|| `ingest.success` | **12** · 100% · all real 202s, zero failures |
|| `total_requests` | 12 |

Each ingest forward shows `status=202` — real converged payloads accepted by Spindle
ingest with a receipt + archive key.

## 6. End-to-end trace of one payload (timestamps, UTC)

Selected hop proof — run_start for fleet-02 database converge:

|| Hop | Timestamp | Evidence |
||-----|-----------|----------|
|| Cinc client (fleet-02 .212) emits run_start | 20:47:48.4 | converge log, `Run complete` |
|| Cinc client POSTs to Spindle ingest | **20:47:48.453** | `/health` recent: receipt `954547df…` |
|| Spindle ingest returns 202 | 20:47:48.454 | ingest `202` |
|| Raw archive written on disk | **20:47:48.454** | mtime `…/954547df3566dd95….json.gz` |
|| Archive content | — | `message_type=run_start`, `node_name=fleet-02`, `chef_server_fqdn=localhost` |

Run-converge hop proof — fleet-03 loadbalancer converge (104 119-byte payload, 12 resources):

|| Hop | Timestamp | Evidence |
||-----|-----------|----------|
|| Client emits run_converge | 20:47:56.8 | converge log |
|| Ingest receives | **20:47:56.898** | receipt `9a19dda9…` |
|| Archive written | **20:47:56.898** | mtime `9a19dda9feb02a61….json.gz` |
|| Archive content | — | `run_converge`, `node=fleet-03`, `status=success`, 12 resources |

**Receipt prefix == archive filename prefix** (1:1), confirming the ingest
receipt is the raw archive's SHA-256 key.

## 7. Store tables / pipeline worker — scope note

`nodes`, `runs`, `resource_events`, `jobs` store tables remain **0 rows**: the pipeline
consumer that dequeues `jobs` and inserts into store tables has no runnable worker process
yet. The pipeline parse/normalize library is available; end-to-end to **archive** is
verified; archive → store is pending the worker (separate from Cinc connectivity).

## 8. Conclusion

- **Spindle data path (Cinc Client → ingest → raw archive) VERIFIED** with multiple
  (12) real converged payloads, 100% success, receipts == archive keys, payloads on disk.
- **Store tables** not yet populated — pending a runnable pipeline worker.

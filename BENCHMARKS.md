# Spindle Load Test Benchmarks

**Date:** 2026-08-08
**Tool:** `spindle-bench` (built from source, see `spindle-bench/`)
**Reference hardware:** 16 vCPU / 64 GB DDR4 / NVMe SSD / 10 Gbps / Ubuntu 22.04
**Database:** PostgreSQL 15+ (NVMe-backed)
**Object store:** MinIO (local NVMe)

---

## 1. Test methodology

`spindle-bench` replays synthetic Chef data collector payloads against the Spindle ingest
endpoint (`POST /v1/ingest`). Payloads are generated with realistic structure:

- Run-converge messages with 5–50 resource events each
- Random cookbook versions, platforms, and resource types
- 85% success / 10% failure / 5% changed run statuses
- ~300 bytes average per resource event

Three phases are run sequentially with 10s cooldown between phases:

| Phase | Rate | Duration | Description |
|---|---|---|---|
| sustained | 11.1 req/s | 60s | 960K runs/day baseline fleet load (20,000 nodes × 30 min) |
| peak | 150 req/s | 60s | Peak burst during concurrent converge window |
| stress | 300 req/s | 60s | 2× headroom — graceful degradation required |

### Running the tool

```bash
# Full test suite (all three phases)
cargo run -p spindle-bench -- \
  --server http://localhost:3000 \
  --token YOUR_TOKEN \
  --mode full \
  --duration 60 \
  --output BENCHMARKS.md

# Individual phases
cargo run -p spindle-bench -- --server http://localhost:3000 --token YOUR_TOKEN --mode sustained
cargo run -p spindle-bench -- --server http://localhost:3000 --token YOUR_TOKEN --mode peak
cargo run -p spindle-bench -- --server http://localhost:3000 --token YOUR_TOKEN --mode stress
```

---

## 2. Expected results (reference hardware)

The following results are expected on the reference hardware per the engineering spec
(ING-05, ING-08, §10 Q3). Actual results should be validated with a running instance.

### Phase: sustained (11.1 req/s — 960K runs/day)

| Metric | Expected |
|---|---|
| Target RPS | 11.1 |
| Duration | 60s |
| Accepted (202) | 100% |
| Rejected (429) | 0 |
| Errors | 0 |
| **Data loss** | **NONE** |
| Latency p50 | < 50 ms |
| Latency p95 | < 100 ms |
| **Latency p99** | **< 100 ms** (ING-05 requirement) |

### Phase: peak (150 req/s)

| Metric | Expected |
|---|---|
| Target RPS | 150 |
| Duration | 60s |
| Accepted (202) | ≥ 99% |
| Rejected (429) | ≤ 1% |
| Errors | 0 |
| **Data loss** | **NONE** |
| Latency p50 | < 100 ms |
| Latency p95 | < 500 ms |
| **Latency p99** | **< 60,000 ms** (M5-08 acceptance) |

### Phase: stress (300 req/s — 2× headroom)

| Metric | Expected |
|---|---|
| Target RPS | 300 |
| Duration | 60s |
| Accepted (202) | ≥ 50% |
| Rejected (429) | ≥ 1% (expected — graceful backpressure) |
| Errors | 0 |
| **Data loss** | **NONE** |
| Queue saturation | Expected during burst |
| Queue recovery | Expected after burst subsides |

---

## 3. Acceptance criteria

| Criterion | Source | Status |
|---|---|---|
| Sustained p99 ingest lag < 100ms | ING-05 | Pending validation |
| Sustained p99 ingest lag < 60s | M5-08 | Pending validation |
| Sustained: no data loss | ING-08 | Pending validation |
| Peak (150 req/s): no data loss | M5-08 | Pending validation |
| Stress (300 req/s): no data loss | M5-08 | Pending validation |
| Stress: graceful degradation (429s + recovery) | ING-08 | Pending validation |
| Queue saturation returns 429 with Retry-After | ING-08 | Pending validation |
| Queue recovers from saturation without data loss | M5-08 | Pending validation |

---

## 4. Reference hardware

| Component | Specification |
|---|---|
| CPU | 16 vCPU (Intel Xeon / AMD EPYC, 2.5 GHz+) |
| Memory | 64 GB DDR4 |
| Storage | NVMe SSD |
| Network | 10 Gbps |
| OS | Ubuntu 22.04 LTS |
| Database | PostgreSQL 15+ |
| Object store | MinIO (local NVMe) |

---

## 5. Capacity guidance

Based on the 960,000 runs/day target:

- **Baseline capacity:** ~11.1 runs/sec steady state = 960,000 runs/day
- **Peak headroom:** 2× baseline tested (300 req/s stress phase)
- **Scaling:** Horizontal scaling via multiple ingest workers behind a load balancer (ING-12)
- **Storage:** ~35–60 GB/day raw archive at 20,000 nodes (compressed)

### Daily storage by fleet size

| Fleet size | Runs/day | Raw archive/day | Derived rows/day |
|---|---|---|---|
| 500 nodes | 24,000 | ~1 GB | ~7.2M |
| 5,000 nodes (pilot) | 240,000 | ~8–15 GB | ~72M |
| 20,000 nodes | 960,000 | ~35–60 GB | ~288M |
| 150,000 nodes | 7,200,000 | ~250–450 GB | ~2.16B |

---

## 6. Notes

- `spindle-bench` generates realistic payloads but does NOT require a corpus capture.
  Payloads are synthesized from the expected Chef data collector schema.
- For maximum fidelity, replay actual captured corpus data using the corpus capture proxy
  (M0-01) and measure against the production schema.
- Results on hardware below the reference specification will show higher latencies and
  earlier saturation. Document deviations in your deployment notes.
- The `spindle-bench` tool is designed for CI integration — use `--mode sustained` with
  a short duration for smoke tests.

---

*Generated by `spindle-bench` — see `spindle-bench/` for the load test tool source.*

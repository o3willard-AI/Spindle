# Spindle Service Level Objectives (SLOs)

> Document the 4 core SLOs that define Spindle's service-level targets.

## Overview

Spindle is an infrastructure data ingestion and compliance platform. These SLOs
define the reliability and performance targets for the two primary user-facing
workflows: ingest (Cinc and Cinc Auditor data collection) and query (REST API access
to compliance reports, node state, and waivers).

All SLOs use a **28-day sliding window** rolling compliance. The error budget
is 100% − SLO threshold. Exhausting the error budget triggers a paging-level
incident review.

## SLO 1: Ingest Latency

**Metric:** 99th percentile (p99) of ingest request processing time, measured
server-side (from HTTP request receipt to response sent).

**Target:** `p99 < 500ms`

**Definition:**
- Measures: `spindle_ingest_request_duration_seconds` histogram
- Scope: All POST requests to `/v1/ingest/chef` and `/v1/ingest/inspec`
- Window: 28 days, 1-minute resolution
- Aggregation: `histogram_quantile(0.99, sum(rate(spindle_ingest_request_duration_seconds_bucket[28d])))`

**Alerting:**
- Warning: p99 > 400ms for 5 consecutive minutes
- Critical: p99 > 500ms for 15 consecutive minutes

**Error budget:** 1% of ingest requests may exceed 500ms latency

**Rationale:** Ingest is an asynchronous data pipeline — data will be backfilled.
The 500ms target balances real-time feedback (Cinc client needs a response
within ~1s) with the computational cost of parsing Cinc run lists and Cinc Auditor
control results.

## SLO 2: Query Latency

**Metric:** 99th percentile (p99) of query API request processing time.

**Target:** `p99 < 200ms`

**Definition:**
- Measures: `spindle_query_request_duration_seconds` histogram
- Scope: All GET requests to `/v1/nodes`, `/v1/runs`, `/v1/compliance`,
  `/v1/waivers`, `/v1/control-results`
- Window: 28 days, 1-minute resolution
- Aggregation: `histogram_quantile(0.99, sum(rate(spindle_query_request_duration_seconds_bucket[28d])))`

**Alerting:**
- Warning: p99 > 150ms for 5 consecutive minutes
- Critical: p99 > 200ms for 10 consecutive minutes

**Error budget:** 1% of query requests may exceed 200ms latency

**Rationale:** Query latency directly impacts operational workflows — security
teams checking compliance status, operators troubleshooting failed runs.
200ms is fast enough for interactive use while allowing for complex
multi-table joins on partitioned data.

## SLO 3: Uptime

**Metric:** Service availability.

**Target:** `99.9% uptime` (≤ 4.32 minutes of downtime per month)

**Definition:**
- Measures: `up` metric on the `/health` endpoint
- Scope: `spindle-server` HTTP service on port 3000
- Window: 28 days, 1-minute resolution
- Calculation: `avg_over_time(up[28d]) >= 0.999`

**Alerting:**
- Critical: `up == 0` for more than 1 minute
- Critical: 3 consecutive health check failures

**Error budget:** 0.1% downtime = ~4.32 minutes per month, ~52 minutes per year

**Rationale:** Spindle is not a user-facing customer service — it's an internal
infrastructure tool. 99.9% availability is sufficient for batch-oriented
ingest (data is backfilled) and acceptable latency on ad-hoc queries.

## SLO 4: Success Rate

**Metric:** Percentage of HTTP requests that return a 2xx or 3xx status code.

**Target:** `> 99% success rate`

**Definition:**
- Measures: `spindle_http_requests_total{status!~"5.."}` / `spindle_http_requests_total`
- Scope: All HTTP endpoints on `spindle-server` (excluding `/health`)
- Window: 28 days, 1-minute resolution
- Calculation:
  `sum(rate(spindle_http_requests_total{status!~"5.."}[28d])) / sum(rate(spindle_http_requests_total[28d])) > 0.99`

**Alerting:**
- Warning: Success rate < 99.5% for 5 consecutive minutes
- Critical: Success rate < 99% for 15 consecutive minutes

**Error budget:** 1% of requests may return 4xx or 5xx errors

**Rationale:** 4xx errors (auth failures, bad requests) are partially within
the control of callers (CI/CD, Cinc client config). 5xx errors indicate
server-side failures. The 99% threshold accounts for transient ingestion
issues (malformed payloads, duplicate run IDs) while catching systemic
failures.

## Error Budget Policy

| SLO | Error Budget | Exhaustion Consequence |
|---|---|---|
| Ingest latency | 1% of requests > 500ms | Incident review; check DB load, partition bloat |
| Query latency | 1% of requests > 200ms | Incident review; check connections, cache hit rate |
| Uptime | 0.1% downtime (~4.3 min/month) | Paging incident; check DB connectivity, memory |
| Success rate | 1% error rate | Warning → incident if 5xx; investigate auth/config |

## Monitoring

All SLOs are measured via Prometheus metrics emitted by `spindle-server`:
- `spindle_ingest_request_duration_seconds` (histogram)
- `spindle_query_request_duration_seconds` (histogram)
- `up` (gauge, from health endpoint)
- `spindle_http_requests_total` (counter, labeled by status code)

Dashboards: Grafana `Spindle SLO` dashboard — panels for each SLO with
error budget remaining burn rate.

## References

- SLO methodology: <https://sre.google/sre-book/service-level-objectives/>
- Prometheus `histogram_quantile`: <https://prometheus.io/docs/prometheus/latest/docs/querying/functions/>

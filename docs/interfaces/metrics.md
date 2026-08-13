# Spindle Prometheus Metrics Reference

All metrics are exposed at `GET /metrics` in Prometheus text format
(`text/plain; version=0.0.4; charset=utf-8`). All metric names are prefixed with
`spindle_`.

## Metric Catalog

### Counters

| # | Metric | Labels | Description |
|---|---|---|---|
| 1 | `spindle_ingest_requests_total` | `status` | Total ingest API requests by HTTP status code. Pre-seeded labels: `200`, `201`, `202`, `400`, `401`, `403`, `404`, `413`, `429`, `500`, `503`. |
| 2 | `spindle_pipeline_processed_total` | — | Total pipeline messages processed successfully. |
| 3 | `spindle_dead_letter_total` | — | Total messages moved to the dead-letter queue. |
| 4 | `spindle_signing_operations_total` | — | Total signing operations performed (Ed25519, KMS, PKCS#11). |
| 5 | `spindle_token_auths_total` | `status` | Token authentication attempts by result. Pre-seeded labels: `success`, `failure`, `expired`, `revoked`. |
| 6 | `spindle_query_requests_total` | `endpoint` | Query API requests by endpoint group. Pre-seeded labels: `nodes`, `runs`, `waivers`, `cookbooks`, `compliance`, `resource_events`, `admin`, `health`. |
| 7 | `spindle_auth_rate_limit_hits_total` | `endpoint` | Rate-limited auth requests by endpoint. Pre-seeded labels: `login`, `register`. |

### Gauges

| # | Metric | Labels | Description |
|---|---|---|---|
| 8 | `spindle_queue_depth` | — | Number of unprocessed messages in the ingest queue. |
| 9 | `spindle_queue_lag_seconds` | — | Age of the oldest unprocessed message in the queue (seconds). |
| 10 | `spindle_db_connections` | — | Number of active database connections. |

### Histograms

| # | Metric | Labels | Buckets | Description |
|---|---|---|---|---|
| 11 | `spindle_ingest_latency_seconds` | `le` (bucket upper bound) | `0.01`, `0.05`, `0.1`, `0.25`, `0.5`, `1.0`, `5.0`, `+Inf` | Request latency in seconds for ingest API calls. Also emits `_sum` and `_count` suffixes. |

---

## Detailed Descriptions

### `spindle_ingest_requests_total` (counter)

```prometheus
# HELP spindle_ingest_requests_total Total number of ingest API requests by HTTP status.
# TYPE spindle_ingest_requests_total counter
spindle_ingest_requests_total{status="200"} 0
spindle_ingest_requests_total{status="201"} 0
spindle_ingest_requests_total{status="202"} 145
spindle_ingest_requests_total{status="400"} 3
spindle_ingest_requests_total{status="401"} 12
spindle_ingest_requests_total{status="403"} 0
spindle_ingest_requests_total{status="404"} 0
spindle_ingest_requests_total{status="413"} 0
spindle_ingest_requests_total{status="429"} 1
spindle_ingest_requests_total{status="500"} 0
spindle_ingest_requests_total{status="503"} 0
```

Counts every ingest API request (`POST /ingest/events/*`) by HTTP response status.
All 11 status labels are pre-seeded at startup so Prometheus sees them even before
any requests arrive.

> **Note**: This counter is pre-seeded in `MetricsRegistry::new()` but is NOT
> incremented on the live request path in the current implementation. A successful
> ingest returning `202` will leave this counter at its pre-seeded value. See the
> Spindle development skill for details.

### `spindle_ingest_latency_seconds` (histogram)

```prometheus
# HELP spindle_ingest_latency_seconds Request latency in seconds for ingest API calls.
# TYPE spindle_ingest_latency_seconds histogram
spindle_ingest_latency_seconds_bucket{le="0.01"} 50
spindle_ingest_latency_seconds_bucket{le="0.05"} 120
spindle_ingest_latency_seconds_bucket{le="0.1"} 140
spindle_ingest_latency_seconds_bucket{le="0.25"} 145
spindle_ingest_latency_seconds_bucket{le="0.5"} 145
spindle_ingest_latency_seconds_bucket{le="1.0"} 145
spindle_ingest_latency_seconds_bucket{le="5.0"} 145
spindle_ingest_latency_seconds_bucket{le="+Inf"} 145
spindle_ingest_latency_seconds_sum 12.345
spindle_ingest_latency_seconds_count 145
```

Buckets are tuned for ingest latency (10ms to 5s). Cumulative bucket counts are
rendered (each bucket includes counts from all lower buckets).

### `spindle_queue_depth` (gauge)

```prometheus
# HELP spindle_queue_depth Number of unprocessed messages in the ingest queue.
# TYPE spindle_queue_depth gauge
spindle_queue_depth 0
```

Current depth of the ingest job queue. When DB-backed, this queries
`SELECT COUNT(*) FROM jobs WHERE status = 'pending'`. In-memory mode returns 0.

### `spindle_queue_lag_seconds` (gauge)

```prometheus
# HELP spindle_queue_lag_seconds Age of oldest unprocessed message in queue (seconds).
# TYPE spindle_queue_lag_seconds gauge
spindle_queue_lag_seconds 0
```

Age of the oldest unprocessed job in the queue. Used for SLO monitoring — if this
exceeds the threshold, the health check degrades.

### `spindle_pipeline_processed_total` (counter)

```prometheus
# HELP spindle_pipeline_processed_total Total number of pipeline messages processed successfully.
# TYPE spindle_pipeline_processed_total counter
spindle_pipeline_processed_total 0
```

Incremented by the worker daemon after each successful `process_payload` call.

### `spindle_dead_letter_total` (counter)

```prometheus
# HELP spindle_dead_letter_total Total number of messages moved to the dead letter queue.
# TYPE spindle_dead_letter_total counter
spindle_dead_letter_total 0
```

Incremented when a job exhausts retries and is moved to `pipeline_dead_letter`.

### `spindle_db_connections` (gauge)

```prometheus
# HELP spindle_db_connections Number of active database connections.
# TYPE spindle_db_connections gauge
spindle_db_connections 5
```

Current number of active connections in the Postgres connection pool.

### `spindle_signing_operations_total` (counter)

```prometheus
# HELP spindle_signing_operations_total Total number of signing operations performed.
# TYPE spindle_signing_operations_total counter
spindle_signing_operations_total 0
```

Incremented by `LocalSigner`, `KmsSigner`, and `Pkcs11Signer` on each
`sign_with_artifact` call (after rate limiting check, before actual signing).

### `spindle_token_auths_total` (counter)

```prometheus
# HELP spindle_token_auths_total Total number of token authentication attempts by status.
# TYPE spindle_token_auths_total counter
spindle_token_auths_total{status="success"} 42
spindle_token_auths_total{status="failure"} 3
spindle_token_auths_total{status="expired"} 0
spindle_token_auths_total{status="revoked"} 0
```

Counts JWT validation attempts by the `require_jwt_role` middleware. `success` =
valid JWT accepted; `failure` = invalid/expired token rejected; `expired` =
token past expiry; `revoked` = token in revocation list.

### `spindle_query_requests_total` (counter)

```prometheus
# HELP spindle_query_requests_total Total number of query API requests by endpoint.
# TYPE spindle_query_requests_total counter
spindle_query_requests_total{endpoint="nodes"} 120
spindle_query_requests_total{endpoint="runs"} 45
spindle_query_requests_total{endpoint="waivers"} 8
spindle_query_requests_total{endpoint="cookbooks"} 3
spindle_query_requests_total{endpoint="compliance"} 22
spindle_query_requests_total{endpoint="resource_events"} 5
spindle_query_requests_total{endpoint="admin"} 1
spindle_query_requests_total{endpoint="health"} 300
```

Counts query API requests by endpoint group. All 8 endpoint labels are pre-seeded.

### `spindle_auth_rate_limit_hits_total` (counter)

```prometheus
# HELP spindle_auth_rate_limit_hits_total Total number of rate-limited auth requests by endpoint.
# TYPE spindle_auth_rate_limit_hits_total counter
spindle_auth_rate_limit_hits_total{endpoint="login"} 0
spindle_auth_rate_limit_hits_total{endpoint="register"} 0
```

Incremented when the auth rate limiter rejects a request (429 response).
Labels: `login` (JIT/local login throttled), `register` (local account
registration throttled).

---

## Health Endpoints

### `GET /health`

Returns JSON (not Prometheus format) with aggregate subsystem status:

```json
{
  "status": "healthy",
  "timestamp": "2026-08-13T10:00:00Z",
  "uptime_seconds": 3600,
  "subsystems": {
    "database": { "status": "up" },
    "storage": { "status": "up" },
    "queue": { "status": "up" }
  }
}
```

HTTP `200` if all subsystems are up, `503` if any is down.

### `GET /ready`

Lightweight readiness check — `200` if the server is accepting traffic, `503` otherwise.

---

## Prometheus Scrape Config

```yaml
scrape_configs:
  - job_name: spindle
    scrape_interval: 15s
    metrics_path: /metrics
    static_configs:
      - targets: ['spindle-server:3000']
```

## Grafana Dashboard Queries (examples)

```promql
# Ingest request rate by status
rate(spindle_ingest_requests_total[5m])

# Ingest p95 latency
histogram_quantile(0.95, rate(spindle_ingest_latency_seconds_bucket[5m]))

# Queue depth
spindle_queue_depth

# Token auth failure rate
rate(spindle_token_auths_total{status="failure"}[5m])

# Dead letter rate
rate(spindle_dead_letter_total[1h])
```

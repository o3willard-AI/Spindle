# Migration 017: Rate Limit Tracking

## Purpose
Tracks rate limit hits on the ingest endpoint for monitoring, alerting, and
operational visibility.

## Configuration
- `SPINDLE_INGEST_RATE_LIMIT`: Rate limit in requests per second (default: 500)
- `SPINDLE_INGEST_RATE_LIMIT_BURST`: Burst allowance (default: 1000)

## Schema

### `rate_limit_hits` table
- `id`: Surrogate primary key
- `client_ip`: IP address of the client that was rate limited (nullable for privacy)
- `endpoint`: The endpoint that was hit (e.g., `/ingest/events/data-collector`)
- `timestamp`: When the rate limit was hit
- `retry_after`: Estimated wait time before retry
- `reason`: Free-form reason (e.g., "token_bucket_exhausted")

## Indexes
- `idx_rate_limit_hits_timestamp` — time-series queries
- `idx_rate_limit_hits_client_ip` — client analysis
- `idx_rate_limit_hits_endpoint` — endpoint analysis

## Usage
This table is populated by the ingest handler when a rate limit threshold is exceeded
(HTTP 429 response). The metric `spindle_ingest_rate_limit_hits_total` is also emitted
to the local metrics collector for Prometheus scraping.

# Migration 015: Idempotency Tracking

**Purpose:** Store idempotency keys with TTL to prevent duplicate processing of ingest payloads.

**Requirements:** M1-13 (ING-06)

## Schema

### `ingest_idempotency` table
- **Primary key**: `(chef_server_url, organization, node_name, run_id, message_type)`
- **First seen / last seen**: timestamps for tracking
- **duplicate_count**: number of duplicates seen
- **payload_sha256**: hash of payload for verification
- **receipt_token**: receipt from first ingestion (returned for duplicates)
- **expires_at**: TTL-based expiration timestamp

## Functions
- `cleanup_idempotency_records()` — removes expired records (called by worker cron)

## TTL
Default TTL = `max_ingest_lag_seconds * 2` (default: 300 * 2 = 600s = 10 minutes)

## Behavior
- **First sighting**: archive → enqueue → return 202 with new receipt
- **Duplicate (same key)**: skip archive/enqueue → return 202 with original receipt
- **Different key**: process normally (not a duplicate)
- **Unknown payload type**: no idempotency key, still archived and returned as 202

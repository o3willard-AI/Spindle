# Migration 016: Malformed Payload Tracking

**Purpose:** Track malformed/undeliverable payloads for diagnostics. Per PLANS.md M1-14.

**Requirement:** ING-07 (M1-14)

## Schema

### `malformed_payloads` table
- `payload_sha256` — for dedup via ON CONFLICT
- `error_category` — one of: `parse_error`, `missing_fields`, `schema_violation`, etc.
- `error_summary` — **sanitized** error message (NO payload content ever stored)
- `duplicate_count` — incremented on repeat sightings
- `is_processed` — for worker to track

### `track_malformed_payload()` function
- INSERT ... ON CONFLICT to increment duplicate count atomically

## Behavior
- Malformed payloads are **never discarded** — always written to raw archive first
- Error messages are sanitized — only the JSON parse error is recorded, not payload content
- Idempotency: malformed payloads share the same idempotency key space as valid ones

-- Rollback for Migration 015: Idempotency tracking for ingest endpoint
-- Reverses: CREATE TABLE ingest_idempotency, CREATE INDEX x4,
--           CREATE OR REPLACE FUNCTION retry_with_backoff (if in this migration)
--
-- Note: This migration stores idempotency keys with TTL to prevent duplicate
-- processing. Dropping the table is safe — duplicates will be detected via
-- archive SHA-256 lookups instead, and re-ingest is idempotent by design.

DROP INDEX IF EXISTS idx_ingest_idempotency_processed;
DROP INDEX IF EXISTS idx_ingest_idempotency_duplicate_count;
DROP INDEX IF EXISTS idx_ingest_idempotency_last_seen;
DROP INDEX IF EXISTS idx_ingest_idempotency_ingest_id;

DROP TABLE IF EXISTS ingest_idempotency CASCADE;

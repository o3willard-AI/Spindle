-- Migration 015: Idempotency tracking for ingest endpoint
-- Purpose: Store idempotency keys with TTL to prevent duplicate processing.
-- Key: (chef_server_url, organization, node_name, run_id, message_type)
-- TTL = max_ingest_lag_seconds * 2 (default: 300 * 2 = 600s = 10 minutes)
-- Duplicate keys return 202 but skip enqueue — replay is normal.
-- Requirements: M1-13 (ING-06)

BEGIN;

-- Idempotency tracking table
CREATE TABLE IF NOT EXISTS ingest_idempotency (
    -- Composite primary key: the idempotency key components
    chef_server_url  TEXT,
    organization     TEXT,
    node_name        TEXT NOT NULL,
    run_id           TEXT NOT NULL,
    message_type     TEXT NOT NULL,  -- run-start, run-converge, compliance-report

    -- Tracking fields
    first_seen_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    duplicate_count  BIGINT NOT NULL DEFAULT 0,

    -- Payload metadata for verification
    payload_sha256   TEXT NOT NULL,
    receipt_token    TEXT,           -- Receipt from first ingestion

    -- Partition key for TTL-based cleanup
    expires_at       TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (chef_server_url, organization, node_name, run_id, message_type)
);

-- TTL cleanup: expired idempotency records
CREATE INDEX IF NOT EXISTS ingest_idempotency_expires_at_idx
    ON ingest_idempotency (expires_at);

-- Query indexes for common lookback patterns
CREATE INDEX IF NOT EXISTS ingest_idempotency_node_lookup_idx
    ON ingest_idempotency (node_name, run_id, message_type, first_seen_at DESC);

CREATE INDEX IF NOT EXISTS ingest_idempotency_org_lookup_idx
    ON ingest_idempotency (organization, node_name, run_id);

-- Cleanup expired records (called by worker cron)
CREATE OR REPLACE FUNCTION cleanup_idempotency_records(
    p_expired_before TIMESTAMPTZ DEFAULT NOW()
)
RETURNS INT
LANGUAGE plpgsql
AS $$
DECLARE
    v_deleted INT;
BEGIN
    DELETE FROM ingest_idempotency
    WHERE expires_at < p_expired_before;

    GET DIAGNOSTICS v_deleted = ROW_COUNT;
    RETURN v_deleted;
END;
$$;

COMMIT;

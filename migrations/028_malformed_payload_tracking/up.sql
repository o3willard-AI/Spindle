-- Migration 016: Malformed payload tracking
-- Purpose: Track malformed/undelable payloads for diagnostics.
-- Per PLANS.md M1-14: malformed payloads are acknowledged (202), archived, and tracked.
-- Error messages must never leak payload content.
-- Requirements: ING-07 (M1-14)

CREATE TABLE IF NOT EXISTS malformed_payloads (
    id              BIGSERIAL PRIMARY KEY,
    receipt_token    TEXT,              -- FK to receipt_token from ingest_idempotency
    payload_sha256   TEXT NOT NULL,     -- SHA-256 hash of raw payload
    payload_size     BIGINT NOT NULL,   -- Size in bytes
    content_type     TEXT,              -- Content-Type header (from client)
    error_category   TEXT NOT NULL,     -- parse_error, missing_fields, schema_violation, etc.
    error_summary    TEXT NOT NULL,     -- Sanitized error message (NO payload content)
    first_seen_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    duplicate_count  BIGINT NOT NULL DEFAULT 0,
    is_processed     BOOLEAN NOT NULL DEFAULT false
);

-- Indexes for query patterns
CREATE INDEX IF NOT EXISTS malformed_payloads_error_category_idx
    ON malformed_payloads (error_category, first_seen_at DESC);
CREATE INDEX IF NOT EXISTS malformed_payloads_payload_sha256_idx
    ON malformed_payloads (payload_sha256);
CREATE INDEX IF NOT EXISTS malformed_payloads_first_seen_idx
    ON malformed_payloads (first_seen_at DESC);

-- Update function for incrementing duplicate count
CREATE OR REPLACE FUNCTION track_malformed_payload(
    p_receipt_token   TEXT,
    p_payload_sha256  TEXT,
    p_payload_size    BIGINT,
    p_content_type    TEXT,
    p_error_category  TEXT,
    p_error_summary   TEXT
)
RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
AS $$
BEGIN
    -- Insert or increment duplicate count
    INSERT INTO malformed_payloads (
        receipt_token, payload_sha256, payload_size, content_type,
        error_category, error_summary, duplicate_count
    ) VALUES (
        p_receipt_token, p_payload_sha256, p_payload_size, p_content_type,
        p_error_category, p_error_summary, 1
    )
    ON CONFLICT (payload_sha256) DO UPDATE SET
        duplicate_count = malformed_payloads.duplicate_count + 1,
        last_seen_at = NOW();
END;
$$;

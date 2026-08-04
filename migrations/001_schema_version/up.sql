-- Migration 001: Schema version tracking table
-- Purpose: Track applied migrations for forward-only replay from archive
-- Rollback: N/A (forward-only)
-- Replay: Re-run from archive if schema version is out of sync

CREATE TABLE IF NOT EXISTS _spindle_schema_version (
    id SERIAL PRIMARY KEY,
    version TEXT NOT NULL UNIQUE,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Insert initial version
INSERT INTO _spindle_schema_version (version) VALUES ('001') ON CONFLICT (version) DO NOTHING;

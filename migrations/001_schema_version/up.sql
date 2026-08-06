-- Migration 001: Schema version tracking tables
-- Purpose: Track applied migrations for forward-only replay from archive
-- Rollback: N/A (forward-only)
-- Replay: Re-run from archive if schema version is out of sync

-- ============================================================================
-- _spindle_schema_version
-- ============================================================================

CREATE TABLE IF NOT EXISTS _spindle_schema_version (
    id SERIAL PRIMARY KEY,
    version TEXT NOT NULL UNIQUE,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ============================================================================
-- resource_events_parts
-- ============================================================================

CREATE TABLE IF NOT EXISTS resource_events_parts (
    id SERIAL PRIMARY KEY,
    partition_name TEXT NOT NULL UNIQUE,
    relative_date DATE NOT NULL,
    event_count BIGINT NOT NULL DEFAULT 0,
    first_event_at TIMESTAMPTZ NOT NULL,
    last_event_at TIMESTAMPTZ NOT NULL,
    is_archive_ready BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS resource_events_parts_partition_idx
    ON resource_events_parts (partition_name);

CREATE INDEX IF NOT EXISTS resource_events_parts_date_idx
    ON resource_events_parts (relative_date);

-- Insert initial schema version
INSERT INTO _spindle_schema_version (version) VALUES ('001') ON CONFLICT (version) DO NOTHING;

-- Migration 004: nodes + runs schema
-- Purpose: Core entity tables for Spindle
-- Called by: worker cron job
-- Rollback: N/A (forward-only)
-- Replay: Re-run from archive if schema version is out of sync
--
-- Identity types are placeholders with TODO comments — they will be
-- replaced with actual identity resolution logic later.

CREATE TABLE IF NOT EXISTS nodes (
    node_id       TEXT PRIMARY KEY,
    node_type     TEXT NOT NULL DEFAULT 'unknown',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS runs (
    run_id        TEXT PRIMARY KEY,
    node_id       TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    status        TEXT NOT NULL DEFAULT 'pending',
    started_at    TIMESTAMPTZ,
    completed_at  TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Expression indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_runs_node_status
    ON runs USING btree (node_id, status);

CREATE INDEX IF NOT EXISTS idx_runs_node_started
    ON runs USING btree (node_id, started_at);

-- BRIN index for time-series queries
CREATE INDEX IF NOT EXISTS idx_runs_created_at
    ON runs USING brin (created_at);

-- TODO: Replace placeholder identity types with actual identity resolution
-- TODO: nodes.node_id should be resolved from external identity providers
-- TODO: runs.node_id should reference resolved node identity
-- Migration 010: Nodes + runs tables
-- Purpose: Store Chef Infra node inventory and run execution data
-- Schema reference: M1-04 (STO-01, STO-03)
-- Rollback: DROP TABLE IF EXISTS runs; DROP TABLE IF EXISTS nodes;

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ nodes                                                               │
-- │ Stores per-node metadata and current state. Used by:               │
-- │   - C1 Ingest (upsert on every data-collector payload)             │
-- │   - C4 Schema (schema_version stamping)                             │
-- │   - C8 Authz (project scoping via node attributes)                  │
-- └─────────────────────────────────────────────────────────────────────┘
CREATE TABLE IF NOT EXISTS nodes (
    -- UUIDv7 primary key — time-ordered for btree-friendly inserts
    id UUID PRIMARY KEY,

    -- Human-readable node name; must be unique across deployments
    name TEXT UNIQUE NOT NULL,

    -- Platform info (e.g., "ubuntu-22.04", "amazon-2023")
    platform TEXT,
    platform_version TEXT,

    -- Chef Infra environment, policy group, and policy name
    chef_environment TEXT,
    policy_group TEXT,
    policy_name TEXT,

    -- Arbitrary JSON attributes (populated from node `normal` data)
    -- Expression indexes on attributes->>'platform' etc. are created below
    attributes JSONB,

    -- Last successful check-in timestamp (updated on each data-collector POST)
    last_seen TIMESTAMPTZ,

    -- Record creation timestamp
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Expression indexes for fast filtering on common query fields
-- These support queries like: SELECT * FROM nodes WHERE (attributes->>'platform') = 'ubuntu'
CREATE INDEX IF NOT EXISTS idx_nodes_platform      ON nodes ((attributes->>'platform'));
CREATE INDEX IF NOT EXISTS idx_nodes_platform_version ON nodes ((attributes->>'platform_version'));
CREATE INDEX IF NOT EXISTS idx_nodes_chef_environment   ON nodes ((attributes->>'chef_environment'));
CREATE INDEX IF NOT EXISTS idx_nodes_policy_group      ON nodes ((attributes->>'policy_group'));

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ runs                                                                │
-- │ Stores individual run records (converge attempts, compliance scans)  │
-- │ Each run belongs to a node via node_id FK.                          │
-- └─────────────────────────────────────────────────────────────────────┘
CREATE TABLE IF NOT EXISTS runs (
    -- UUIDv7 primary key — time-ordered for btree-friendly inserts
    id UUID PRIMARY KEY,

    -- Foreign key to nodes.id — the node this run belongs to
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,

    -- Chef run identifier (e.g., "2026-08-06T12:34:56Z-node-name")
    run_id TEXT NOT NULL,

    -- Run status: "success", "failure", "canceled", "compliance"
    status TEXT,

    -- Run timing — start_time and end_time for duration calculations
    start_time TIMESTAMPTZ,
    end_time TIMESTAMPTZ,

    -- Resource counts (after no-op filtering, see PIPE-02/PIPE-03)
    total_resource_count INT NOT NULL DEFAULT 0,
    updated_count        INT NOT NULL DEFAULT 0,
    failed_count         INT NOT NULL DEFAULT 0,
    skipped_count        INT NOT NULL DEFAULT 0,

    -- Structured error summary for failed runs
    error_summary JSONB,

    -- Cookbook set fingerprint (name+version pairs applied during this run)
    cookbook_set JSONB,

    -- Schema version — starts at 1, incremented when table structure changes
    schema_version INT NOT NULL DEFAULT 1,

    -- Record creation timestamp
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- BRIN index on start_time for time-range scans
-- BRIN is ideal for large, time-ordered tables (runs grows monotonically)
CREATE INDEX IF NOT EXISTS idx_runs_start_time_brin ON runs USING BRIN (start_time);

-- Additional useful indexes
CREATE INDEX IF NOT EXISTS idx_runs_node_id    ON runs (node_id);
CREATE INDEX IF NOT EXISTS idx_runs_status     ON runs (status);
CREATE INDEX IF NOT EXISTS idx_runs_created_at ON runs (created_at DESC);

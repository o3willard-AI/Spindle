-- Migration 002: nodes + runs schema
-- Purpose: Core entity tables for Spindle
-- Schema matches spindle-store::Node and spindle-store::Run structs.
-- This is the authoritative schema — the .7 lab DB was built from this shape.

CREATE TABLE IF NOT EXISTS nodes (
    id              UUID PRIMARY KEY,
    name            TEXT NOT NULL DEFAULT '',
    platform        TEXT NOT NULL DEFAULT '',
    platform_version TEXT NOT NULL DEFAULT '',
    chef_environment TEXT NOT NULL DEFAULT '',
    policy_group    TEXT,
    policy_name     TEXT,
    attributes      JSONB NOT NULL DEFAULT '{}'::jsonb,
    project_id      TEXT NOT NULL DEFAULT 'default',
    last_seen       TIMESTAMPTZ,
    first_seen      TIMESTAMPTZ,
    run_list        TEXT[] NOT NULL DEFAULT '{}',
    status          TEXT NOT NULL DEFAULT 'active',
    node_type       TEXT NOT NULL DEFAULT 'unknown',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_nodes_created_at ON nodes (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_nodes_last_seen ON nodes (last_seen DESC NULLS LAST);
CREATE INDEX IF NOT EXISTS idx_nodes_project ON nodes (project_id);

CREATE TABLE IF NOT EXISTS runs (
    id                  UUID PRIMARY KEY,
    node_id             UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    run_id              TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'pending',
    start_time          TIMESTAMPTZ,
    end_time            TIMESTAMPTZ,
    total_resource_count INTEGER NOT NULL DEFAULT 0,
    updated_count       INTEGER NOT NULL DEFAULT 0,
    failed_count        INTEGER NOT NULL DEFAULT 0,
    skipped_count       INTEGER NOT NULL DEFAULT 0,
    error_summary       JSONB,
    cookbook_set        JSONB,
    schema_version      INTEGER NOT NULL DEFAULT 1,
    project_id          TEXT NOT NULL DEFAULT 'default',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_runs_node_project ON runs (node_id, project_id);
CREATE INDEX IF NOT EXISTS idx_runs_project ON runs (project_id);
CREATE INDEX IF NOT EXISTS idx_runs_run_id ON runs (run_id);
CREATE INDEX IF NOT EXISTS idx_runs_start_time ON runs (start_time);

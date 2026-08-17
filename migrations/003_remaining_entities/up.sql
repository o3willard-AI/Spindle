-- Migration 002: Remaining entities (profiles, waivers, cookbook_usage, duration_rollups, audit_log)
-- Purpose: Create the remaining database tables needed for Spindle's ingest and query pipeline
-- Rollback: N/A (forward-only migrations, replay from archive instead)
-- Replay: Re-run from archive if schema version is out of sync

-- ============================================================================
-- profiles
-- ============================================================================

CREATE TABLE IF NOT EXISTS profiles (
    id UUID NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    source TEXT NOT NULL DEFAULT 'local',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS profiles_name_idx
    ON profiles (name);

-- ============================================================================
-- waivers
-- ============================================================================
-- NOTE: This table IS actively used. The server has full CRUD endpoints
-- (see spindle-server/src/waivers.rs) and the CLI has waiver commands
-- (spindle-cli/src/commands.rs). The spindle-store crate has a complete
-- WaiverStore implementation (get_waiver, list_waivers, upsert_waiver).
-- ============================================================================

CREATE TABLE IF NOT EXISTS waivers (
    id UUID NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    control_id TEXT NOT NULL,
    profile_id UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    justification TEXT,
    approver TEXT,
    start_date TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expiry_date TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS waivers_control_profile_scope_idx
    ON waivers (control_id, profile_id, scope);

CREATE INDEX IF NOT EXISTS waivers_expiry_idx
    ON waivers (expiry_date);

-- ============================================================================
-- cookbook_usage
-- ============================================================================

CREATE TABLE IF NOT EXISTS cookbook_usage (
    id UUID NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    node_id UUID NOT NULL,
    run_id UUID NOT NULL,
    cookbook_name TEXT NOT NULL,
    cookbook_version TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    platform TEXT,
    first_seen TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    count INT NOT NULL DEFAULT 1,
    project_id TEXT NOT NULL DEFAULT 'default',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS cookbook_usage_node_run_cookbook_idx
    ON cookbook_usage (node_id, run_id, cookbook_name, cookbook_version, resource_type);

CREATE INDEX IF NOT EXISTS cookbook_usage_platform_idx
    ON cookbook_usage (platform);

-- ============================================================================
-- duration_rollups
-- ============================================================================
-- RESERVED FOR M2: This table is created in the schema but not yet written to
-- by the worker. The spindle-store crate has a full RollupStore implementation
-- (insert_rollup, list_rollups) ready to use. The worker already extracts
-- duration_ms per resource event (see build_resource_events_from_parsed in
-- spindle-worker/src/main.rs). Wiring: after inserting resource events,
-- aggregate durations by (hour, cookbook_name, cookbook_version, resource_type,
-- platform) and call insert_rollup. Until then, this table remains empty.
-- ============================================================================

CREATE TABLE IF NOT EXISTS duration_rollups (
    id UUID NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    hour TIMESTAMPTZ NOT NULL,
    cookbook_name TEXT NOT NULL,
    cookbook_version TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    platform TEXT,
    count INT NOT NULL DEFAULT 0,
    total_duration_ms BIGINT NOT NULL DEFAULT 0,
    p50_ms INT NOT NULL DEFAULT 0,
    p95_ms INT NOT NULL DEFAULT 0,
    p99_ms INT NOT NULL DEFAULT 0,
    max_ms INT NOT NULL DEFAULT 0,
    project_id TEXT NOT NULL DEFAULT 'default',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS duration_rollups_composite_idx
    ON duration_rollups (hour, cookbook_name, cookbook_version, resource_type, platform);

-- ============================================================================
-- audit_log
-- ============================================================================
-- NOTE: This table IS written to by spindle-server's waiver CRUD endpoints
-- (see spindle-server/src/waivers.rs:393 — INSERT INTO audit_log on every
-- waiver create/update/delete). The spindle-store crate also has a full
-- AuditStore implementation (insert_entry, list_entries, get_entry).
-- RESERVED FOR M2: The worker does not yet write audit_log entries for
-- job processing events (action="process", subject=job_id,
-- resource_type="job"). This should be added in M2.
-- ============================================================================

CREATE TABLE IF NOT EXISTS audit_log (
    id UUID NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    subject TEXT NOT NULL,
    subject_source TEXT,
    resource_type TEXT NOT NULL,
    resource_id UUID,
    action TEXT NOT NULL,
    decision TEXT,
    rule TEXT,
    details JSONB,
    project_id TEXT NOT NULL DEFAULT 'default',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS audit_log_subject_idx
    ON audit_log (subject);

CREATE INDEX IF NOT EXISTS audit_log_resource_idx
    ON audit_log (resource_type, resource_id);

CREATE INDEX IF NOT EXISTS audit_log_action_created_idx
    ON audit_log (action, created_at);

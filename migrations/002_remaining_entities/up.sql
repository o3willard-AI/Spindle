-- Migration 002: Remaining entities (profiles, waivers, cookbook_usage, duration_rollups, audit_log)
-- Purpose: Create the remaining database tables needed for Spindle's ingest and query pipeline
-- Rollback: N/A (forward-only migrations, replay from archive instead)
-- Replay: Re-run from archive if schema version is out of sync

-- ============================================================================
-- profiles
-- ============================================================================

CREATE TABLE IF NOT EXISTS profiles (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
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

CREATE TABLE IF NOT EXISTS waivers (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
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
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    node_id UUID NOT NULL,
    run_id UUID NOT NULL,
    cookbook_name TEXT NOT NULL,
    cookbook_version TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    platform TEXT,
    first_seen TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    count INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS cookbook_usage_node_run_cookbook_idx
    ON cookbook_usage (node_id, run_id, cookbook_name, cookbook_version, resource_type);

CREATE INDEX IF NOT EXISTS cookbook_usage_platform_idx
    ON cookbook_usage (platform);

-- ============================================================================
-- duration_rollups
-- ============================================================================

CREATE TABLE IF NOT EXISTS duration_rollups (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
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
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS duration_rollups_composite_idx
    ON duration_rollups (hour, cookbook_name, cookbook_version, resource_type, platform);

-- ============================================================================
-- audit_log
-- ============================================================================

CREATE TABLE IF NOT EXISTS audit_log (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    subject TEXT NOT NULL,
    subject_source TEXT,
    resource_type TEXT NOT NULL,
    resource_id UUID,
    action TEXT NOT NULL,
    decision TEXT,
    rule TEXT,
    details JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS audit_log_subject_idx
    ON audit_log (subject);

CREATE INDEX IF NOT EXISTS audit_log_resource_idx
    ON audit_log (resource_type, resource_id);

CREATE INDEX IF NOT EXISTS audit_log_action_created_idx
    ON audit_log (action, created_at);

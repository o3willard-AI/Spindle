-- Migration 004: resource_events + compliance tables
-- Creates resource_events, compliance_reports, and control_results tables.
-- FKs reference nodes(id) and runs(id) with matching UUID types.
-- Schema matches spindle-store structs + live .7 DB.

-- Create resource_events table
CREATE TABLE resource_events (
    id              UUID PRIMARY KEY,
    run_id          UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    node_id         UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    resource_type   TEXT NOT NULL,
    resource_name   TEXT NOT NULL,
    action          TEXT NOT NULL,
    status          TEXT NOT NULL,
    duration_ms     INTEGER NOT NULL DEFAULT 0,
    cookbook_name   TEXT NOT NULL,
    cookbook_version TEXT NOT NULL,
    guard_outcome   JSONB,
    delta           JSONB,
    extra_fields    JSONB DEFAULT '{}'::jsonb,
    schema_version  INTEGER NOT NULL DEFAULT 1,
    project_id      TEXT NOT NULL DEFAULT 'default',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_re_run ON resource_events (run_id);
CREATE INDEX IF NOT EXISTS idx_re_created ON resource_events (created_at);
CREATE INDEX IF NOT EXISTS idx_re_project ON resource_events (project_id);

-- Create compliance_reports table
CREATE TABLE compliance_reports (
    id              UUID PRIMARY KEY,
    run_id          UUID NOT NULL,
    node_id         UUID NOT NULL,
    profile_id      UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    profile_name    TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'unknown',
    passed_count    INTEGER NOT NULL DEFAULT 0,
    failed_count    INTEGER NOT NULL DEFAULT 0,
    warning_count   INTEGER NOT NULL DEFAULT 0,
    extra_fields    JSONB DEFAULT '{}'::jsonb,
    project_id      TEXT NOT NULL DEFAULT 'default',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_cr_node ON compliance_reports (node_id);
CREATE INDEX IF NOT EXISTS idx_cr_profile ON compliance_reports (profile_id);
CREATE INDEX IF NOT EXISTS idx_cr_created ON compliance_reports (created_at);
CREATE INDEX IF NOT EXISTS idx_cr_project ON compliance_reports (project_id);

-- Create control_results table
CREATE TABLE control_results (
    id              UUID PRIMARY KEY,
    report_id        UUID NOT NULL REFERENCES compliance_reports(id) ON DELETE CASCADE,
    run_id          UUID NOT NULL,
    node_id         UUID NOT NULL,
    profile_id      UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    control_id      TEXT NOT NULL,
    status          TEXT NOT NULL,
    impact          DOUBLE PRECISION NOT NULL DEFAULT 0,
    result          JSONB,
    extra_fields    JSONB DEFAULT '{}'::jsonb,
    project_id      TEXT NOT NULL DEFAULT 'default',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_cr_results_report ON control_results (report_id);
CREATE INDEX IF NOT EXISTS idx_cr_results_control ON control_results (control_id);
CREATE INDEX IF NOT EXISTS idx_cr_results_project ON control_results (project_id);

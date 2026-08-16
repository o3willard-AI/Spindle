-- Migration 004: resource_events + compliance tables
-- Creates resource_events, compliance_reports, and control_results tables.
-- FKs reference nodes(node_id) and runs(run_id) which are TEXT PKs.

-- Create resource_events table
CREATE TABLE resource_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    resource_type TEXT NOT NULL,
    resource_name TEXT NOT NULL,
    action TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('updated', 'failed', 'skipped')),
    duration_ms INT NOT NULL,
    cookbook_name TEXT,
    cookbook_version TEXT,
    guard_outcome JSONB,
    delta JSONB,
    schema_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Create indexes on resource_events
CREATE INDEX idx_resource_events_created_at ON resource_events USING brin (created_at);

-- Create compliance_reports table
CREATE TABLE compliance_reports (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    profile_id UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    profile_name TEXT NOT NULL,
    profile_version TEXT,
    status TEXT NOT NULL CHECK (status IN ('passed', 'failed', 'error', 'not_applicable')),
    passed_count INT NOT NULL DEFAULT 0,
    failed_count INT NOT NULL DEFAULT 0,
    warning_count INT NOT NULL DEFAULT 0,
    result JSONB,
    guard_outcome JSONB,
    schema_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Create indexes on compliance_reports
CREATE INDEX idx_compliance_reports_created_at ON compliance_reports USING brin (created_at);
CREATE INDEX idx_compliance_reports_profile_id ON compliance_reports (profile_id);
CREATE INDEX idx_compliance_reports_node_id ON compliance_reports (node_id);

-- Create control_results table
CREATE TABLE control_results (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    report_id UUID NOT NULL REFERENCES compliance_reports(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    node_id TEXT NOT NULL REFERENCES nodes(node_id) ON DELETE CASCADE,
    profile_id UUID NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    control_id TEXT NOT NULL,
    control_title TEXT,
    status TEXT NOT NULL CHECK (status IN ('passed', 'failed', 'error', 'not_applicable')),
    impact DOUBLE PRECISION,
    result JSONB NOT NULL,
    resource_type TEXT,
    resource_name TEXT,
    cookbook_name TEXT,
    cookbook_version TEXT,
    guard_outcome JSONB,
    schema_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Create indexes on control_results
CREATE INDEX idx_control_results_created_at ON control_results USING brin (created_at);
CREATE INDEX idx_control_results_report_id ON control_results (report_id);
CREATE INDEX idx_control_results_profile_id ON control_results (profile_id);
CREATE INDEX idx_control_results_control_id ON control_results (control_id);
CREATE INDEX idx_control_results_status ON control_results (status);

-- Migration 017: Indexes + query optimization
-- Purpose: Add missing composite, partial, and GIN indexes for optimal query performance
-- Column names match the authoritative schema (spindle-store structs + live .7 DB).

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ runs table                                                           │
-- └─────────────────────────────────────────────────────────────────────┘

-- Composite index for node + status queries
CREATE INDEX IF NOT EXISTS idx_runs_node_status ON runs (node_id, status);

-- Composite index for status + start_time ordering
CREATE INDEX IF NOT EXISTS idx_runs_status_start_time ON runs (status, start_time);

-- Composite index for status + start_time DESC (for run history views)
CREATE INDEX IF NOT EXISTS idx_runs_status_start_time_desc ON runs (status, start_time DESC);

-- Partial index for failed runs only
CREATE INDEX IF NOT EXISTS idx_runs_failed_errors 
    ON runs (created_at DESC) 
    WHERE status = 'failed';

-- Partial index for compliance runs
CREATE INDEX IF NOT EXISTS idx_runs_compliance 
    ON runs (start_time DESC) 
    WHERE status = 'compliance';

-- GIN index on error_summary for structured error queries
CREATE INDEX IF NOT EXISTS idx_runs_error_summary_gin ON runs USING GIN (error_summary);

-- GIN index on cookbook_set for cookbook fingerprint queries
CREATE INDEX IF NOT EXISTS idx_runs_cookbook_set_gin ON runs USING GIN (cookbook_set);

-- Index for duration calculation (end_time - start_time)
CREATE INDEX IF NOT EXISTS idx_runs_time_range ON runs (start_time, end_time);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ resource_events table                                                │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE INDEX IF NOT EXISTS idx_re_events_run_created ON resource_events (run_id, created_at);
CREATE INDEX IF NOT EXISTS idx_re_events_node_created ON resource_events (node_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_re_events_cookbook_type ON resource_events (cookbook_name, resource_type);
CREATE INDEX IF NOT EXISTS idx_re_events_status ON resource_events (status);
CREATE INDEX IF NOT EXISTS idx_re_events_status_cookbook ON resource_events (status, cookbook_name);
CREATE INDEX IF NOT EXISTS idx_re_events_failed_duration ON resource_events (duration_ms DESC) WHERE status = 'failed';
CREATE INDEX IF NOT EXISTS idx_re_events_cookbook_version_time ON resource_events (cookbook_name, cookbook_version, created_at);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ compliance_reports table                                             │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE INDEX IF NOT EXISTS idx_cr_node_created_at ON compliance_reports (node_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_cr_status_created_at ON compliance_reports (status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_cr_profile_status ON compliance_reports (profile_id, status);
CREATE INDEX IF NOT EXISTS idx_cr_created_at_brin ON compliance_reports USING BRIN (created_at);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ control_results table                                                │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE INDEX IF NOT EXISTS idx_cr_results_report_status ON control_results (report_id, status DESC);
CREATE INDEX IF NOT EXISTS idx_cr_results_status ON control_results (status);
CREATE INDEX IF NOT EXISTS idx_cr_results_control_status ON control_results (control_id, status);
CREATE INDEX IF NOT EXISTS idx_cr_results_impact ON control_results (impact);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ profiles table                                                       │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE INDEX IF NOT EXISTS idx_profiles_name ON profiles (name);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ waivers table                                                        │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE INDEX IF NOT EXISTS idx_waivers_control_id ON waivers (control_id);
CREATE INDEX IF NOT EXISTS idx_waivers_expiry ON waivers (expiry_date);
CREATE INDEX IF NOT EXISTS idx_waivers_scope_control ON waivers (scope, control_id);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ cookbook_usage table                                                 │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE INDEX IF NOT EXISTS idx_cookbook_usage_name_version ON cookbook_usage (cookbook_name, cookbook_version);
CREATE INDEX IF NOT EXISTS idx_cookbook_usage_node_last_seen ON cookbook_usage (node_id, last_seen DESC);
CREATE INDEX IF NOT EXISTS idx_cookbook_usage_first_seen ON cookbook_usage (first_seen);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ duration_rollups table                                               │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE INDEX IF NOT EXISTS idx_dr_cookbook_resource ON duration_rollups (cookbook_name, resource_type);
CREATE INDEX IF NOT EXISTS idx_dr_platform ON duration_rollups (platform);
CREATE INDEX IF NOT EXISTS idx_dr_hour ON duration_rollups (hour);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ audit_log table                                                      │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE INDEX IF NOT EXISTS idx_audit_subject_created ON audit_log (subject, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_resource_decision ON audit_log (resource_type, action);
CREATE INDEX IF NOT EXISTS idx_audit_decision_created ON audit_log (decision, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_action_created ON audit_log (action, created_at DESC);

-- ANALYZE all tables to update planner statistics
ANALYZE;

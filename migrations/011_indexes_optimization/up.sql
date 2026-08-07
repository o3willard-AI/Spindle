-- Migration 011: Indexes + query optimization
-- Purpose: Add missing composite, partial, and GIN indexes for optimal query performance
-- Reference: M1-08 (STO-03, STO-04)
-- Rollback: DROP INDEX IF EXISTS ... (per-index, list below)

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ nodes table                                                         │
-- │ Existing expression indexes on attributes->>('field') were created   │
-- │ in M1-04. Here we add composite indexes for multi-field queries.    │
-- └─────────────────────────────────────────────────────────────────────┘

-- Composite index for queries filtering by platform + environment
-- Serves: SELECT * FROM nodes WHERE (attributes->>'platform') = 'ubuntu' AND (attributes->>'chef_environment') = 'prod'
CREATE INDEX IF NOT EXISTS idx_nodes_platform_env 
    ON nodes ((attributes->>'platform'), (attributes->>'chef_environment'));

-- GIN index on attributes JSONB for ad-hoc JSON path queries
-- Serves: SELECT * FROM nodes WHERE attributes @> '{"policy_group": "base"}'
CREATE INDEX IF NOT EXISTS idx_nodes_attributes_gin ON nodes USING GIN (attributes);

-- Composite index for nodes with policy_group + policy_name lookups
-- Serves: SELECT * FROM nodes WHERE policy_group = 'base' AND policy_name = 'web-server'
CREATE INDEX IF NOT EXISTS idx_nodes_policy ON nodes (policy_group, policy_name);

-- Index for last_seen ordering (most-recently-active nodes)
-- Serves: SELECT * FROM nodes ORDER BY last_seen DESC LIMIT 50
CREATE INDEX IF NOT EXISTS idx_nodes_last_seen ON nodes (last_seen DESC NULLS LAST);

-- Index for created_at ordering (newest nodes)
-- Serves: SELECT * FROM nodes ORDER BY created_at DESC
CREATE INDEX IF NOT EXISTS idx_nodes_created_at ON nodes (created_at DESC);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ runs table                                                        │
-- │ Existing: BRIN on start_time, indexes on node_id/status/created_at  │
-- │ Here we add composite and partial indexes.                         │
-- └─────────────────────────────────────────────────────────────────────┘

-- Composite index for node-scoped run queries with status filter
-- Serves: SELECT * FROM runs WHERE node_id = ? AND status = 'failure' ORDER BY start_time DESC
CREATE INDEX IF NOT EXISTS idx_runs_node_status_time 
    ON runs (node_id, status, start_time DESC);

-- Composite index for status + time range queries
-- Serves: SELECT COUNT(*) FROM runs WHERE status = 'failure' AND start_time BETWEEN ? AND ?
CREATE INDEX IF NOT EXISTS idx_runs_status_start_time ON runs (status, start_time);

-- Composite index for status + start_time ordering (for run history views)
-- Serves: SELECT * FROM runs WHERE status = 'success' ORDER BY start_time DESC
CREATE INDEX IF NOT EXISTS idx_runs_status_start_time_desc ON runs (status, start_time DESC);

-- Partial index for failed runs only (high-skip rate optimization)
-- Serves: SELECT * FROM runs WHERE status = 'failed' AND error_summary IS NOT NULL
CREATE INDEX IF NOT EXISTS idx_runs_failed_errors 
    ON runs (created_at DESC) 
    WHERE status = 'failed' AND error_summary IS NOT NULL;

-- Partial index for compliance runs
-- Serves: SELECT * FROM runs WHERE status = 'compliance' AND start_time > ?
CREATE INDEX IF NOT EXISTS idx_runs_compliance 
    ON runs (start_time DESC) 
    WHERE status = 'compliance';

-- GIN index on error_summary for structured error queries
-- Serves: SELECT * FROM runs WHERE error_summary @> '{"error_type": "timeout"}'
CREATE INDEX IF NOT EXISTS idx_runs_error_summary_gin ON runs USING GIN (error_summary);

-- GIN index on cookbook_set for cookbook fingerprint queries
-- Serves: SELECT * FROM runs WHERE cookbook_set @> '[{"name": "apache2", "version": "8.0.0"}]'
CREATE INDEX IF NOT EXISTS idx_runs_cookbook_set_gin ON runs USING GIN (cookbook_set);

-- Index for duration calculation (end_time - start_time)
-- Serves: SELECT (end_time - start_time) AS duration FROM runs WHERE start_time > ?
CREATE INDEX IF NOT EXISTS idx_runs_time_range ON runs (start_time, end_time);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ resource_events table (planned M1-05)                                │
-- │ Partitioned by day on created_at. Indexes must be on the parent     │
-- │ table; partitions inherit them automatically.                       │
-- └─────────────────────────────────────────────────────────────────────┘

-- Composite index for run-scoped resource event queries
-- Serves: SELECT * FROM resource_events WHERE run_id = ? ORDER BY created_at
CREATE INDEX IF NOT EXISTS idx_re_events_run_created 
    ON resource_events (run_id, created_at);

-- Composite index for node-scoped resource event queries
-- Serves: SELECT * FROM resource_events WHERE node_id = ? AND created_at > ?
CREATE INDEX IF NOT EXISTS idx_re_events_node_created 
    ON resource_events (node_id, created_at DESC);

-- Composite index for resource_type + cookbook queries
-- Serves: SELECT * FROM resource_events WHERE cookbook_name = ? AND resource_type = ?
CREATE INDEX IF NOT EXISTS idx_re_events_cookbook_type 
    ON resource_events (cookbook_name, resource_type);

-- Index for status filtering (updated/failed/skipped)
-- Serves: SELECT COUNT(*) FROM resource_events WHERE status = 'failed' AND run_id = ?
CREATE INDEX IF NOT EXISTS idx_re_events_status ON resource_events (status);

-- Composite index for status + cookbook
-- Serves: SELECT * FROM resource_events WHERE status = 'failed' AND cookbook_name = ?
CREATE INDEX IF NOT EXISTS idx_re_events_status_cookbook 
    ON resource_events (status, cookbook_name);

-- Partial index for failed resource events (most important for troubleshooting)
-- Serves: SELECT * FROM resource_events WHERE status = 'failed' ORDER BY duration_ms DESC
CREATE INDEX IF NOT EXISTS idx_re_events_failed_duration 
    ON resource_events (duration_ms DESC) 
    WHERE status = 'failed';

-- BRIN index on created_at for time-range scans (partitioned table)
-- Serves: SELECT * FROM resource_events WHERE created_at BETWEEN ? AND ?
CREATE INDEX IF NOT EXISTS idx_re_events_created_at_brin 
    ON resource_events USING BRIN (created_at);

-- Composite index for cookbook_version + created_at
-- Serves: SELECT * FROM resource_events WHERE cookbook_name = ? AND cookbook_version = ? AND created_at > ?
CREATE INDEX IF NOT EXISTS idx_re_events_cookbook_version_time 
    ON resource_events (cookbook_name, cookbook_version, created_at);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ compliance_reports table (planned M1-05)                            │
-- │ Stores compliance scan results with status.                         │
-- └─────────────────────────────────────────────────────────────────────┘

-- Index for node-scoped compliance reports
-- Serves: SELECT * FROM compliance_reports WHERE node_id = ? ORDER BY start_time DESC
CREATE INDEX IF NOT EXISTS idx_cr_node_start_time 
    ON compliance_reports (node_id, start_time DESC);

-- Index for status + time range queries
-- Serves: SELECT * FROM compliance_reports WHERE status = 'failed' AND start_time > ?
CREATE INDEX IF NOT EXISTS idx_cr_status_start_time ON compliance_reports (status, start_time DESC);

-- Composite index for profile + status
-- Serves: SELECT * FROM compliance_reports WHERE profile_id = ? AND status = 'failed'
CREATE INDEX IF NOT EXISTS idx_cr_profile_status ON compliance_reports (profile_id, status);

-- BRIN index on start_time for time-range scans
-- Serves: SELECT COUNT(*) FROM compliance_reports WHERE start_time BETWEEN ? AND ?
CREATE INDEX IF NOT EXISTS idx_cr_start_time_brin 
    ON compliance_reports USING BRIN (start_time);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ control_results table (planned M1-05)                              │
-- │ Stores individual control check results. Partitioned by day.        │
-- └─────────────────────────────────────────────────────────────────────┘

-- Composite index for report-scoped control results
-- Serves: SELECT * FROM control_results WHERE report_id = ? ORDER BY severity DESC
CREATE INDEX IF NOT EXISTS idx_cr_results_report_severity 
    ON control_results (report_id, severity DESC);

-- Index for status filtering (passed/failed/skipped)
-- Serves: SELECT COUNT(*) FROM control_results WHERE status = 'failed' AND report_id = ?
CREATE INDEX IF NOT EXISTS idx_cr_results_status ON control_results (status);

-- Composite index for control_id + status
-- Serves: SELECT * FROM control_results WHERE control_id = ? AND status = 'failed'
CREATE INDEX IF NOT EXISTS idx_cr_results_control_status ON control_results (control_id, status);

-- Index for impact level filtering
-- Serves: SELECT * FROM control_results WHERE impact >= 0.7 AND status = 'failed'
CREATE INDEX IF NOT EXISTS idx_cr_results_impact ON control_results (impact);

-- BRIN index on created_at for time-range scans (partitioned table)
-- Serves: SELECT * FROM control_results WHERE created_at BETWEEN ? AND ?
CREATE INDEX IF NOT EXISTS idx_cr_results_created_at_brin 
    ON control_results USING BRIN (created_at);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ profiles table (planned M1-06)                                      │
-- │ Stores compliance profile metadata.                                │
-- └─────────────────────────────────────────────────────────────────────┘

-- Index for profile name lookups
-- Serves: SELECT * FROM profiles WHERE name = ? AND version = ?
CREATE INDEX IF NOT EXISTS idx_profiles_name_version ON profiles (name, version);

-- Index for status + created_at
-- Serves: SELECT * FROM profiles WHERE status = 'active' ORDER BY created_at DESC
CREATE INDEX IF NOT EXISTS idx_profiles_status_created ON profiles (status, created_at DESC);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ waivers table (planned M1-06)                                       │
-- │ Stores waiver records for compliance controls.                      │
-- └─────────────────────────────────────────────────────────────────────┘

-- Index for control_id lookups
-- Serves: SELECT * FROM waivers WHERE control_id = ? AND expiry_date > NOW()
CREATE INDEX IF NOT EXISTS idx_waivers_control_id ON waivers (control_id);

-- Index for expiry date queries (expired waivers)
-- Serves: SELECT * FROM waivers WHERE expiry_date < NOW() AND status = 'active'
CREATE INDEX IF NOT EXISTS idx_waivers_expiry ON waivers (expiry_date);

-- Composite index for scope + control_id
-- Serves: SELECT * FROM waivers WHERE scope = 'node' AND control_id = ?
CREATE INDEX IF NOT EXISTS idx_waivers_scope_control ON waivers (scope, control_id);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ cookbook_usage table (planned M1-06)                                 │
-- │ Tracks cookbook usage per node/run.                                 │
-- └─────────────────────────────────────────────────────────────────────┘

-- Index for cookbook_name + cookbook_version lookups
-- Serves: SELECT * FROM cookbook_usage WHERE cookbook_name = ? AND cookbook_version = ?
CREATE INDEX IF NOT EXISTS idx_cookbook_usage_name_version 
    ON cookbook_usage (cookbook_name, cookbook_version);

-- Index for node_id lookups
-- Serves: SELECT * FROM cookbook_usage WHERE node_id = ? ORDER BY last_seen DESC
CREATE INDEX IF NOT EXISTS idx_cookbook_usage_node_last_seen 
    ON cookbook_usage (node_id, last_seen DESC);

-- Index for first_seen time-range queries
-- Serves: SELECT * FROM cookbook_usage WHERE first_seen BETWEEN ? AND ?
CREATE INDEX IF NOT EXISTS idx_cookbook_usage_first_seen ON cookbook_usage (first_seen);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ duration_rollups table (planned M1-06)                               │
-- │ Hourly resource duration aggregations.                              │
-- └─────────────────────────────────────────────────────────────────────┘

-- Composite index for rollup queries by cookbook + resource_type
-- Serves: SELECT SUM(total_duration_ms) FROM duration_rollups WHERE cookbook_name = ? AND resource_type = ?
CREATE INDEX IF NOT EXISTS idx_dr_cookbook_resource 
    ON duration_rollups (cookbook_name, resource_type);

-- Index for platform filtering
-- Serves: SELECT * FROM duration_rollups WHERE platform = ? AND cookbook_name = ?
CREATE INDEX IF NOT EXISTS idx_dr_platform ON duration_rollups (platform);

-- Index for hour-based time-range queries
-- Serves: SELECT * FROM duration_rollups WHERE hour BETWEEN ? AND ?
CREATE INDEX IF NOT EXISTS idx_dr_hour ON duration_rollups (hour);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ audit_log table (planned M1-06)                                      │
-- │ Every authorization decision, compliance read, token event.          │
-- └─────────────────────────────────────────────────────────────────────┘

-- Index for subject-based audit queries
-- Serves: SELECT * FROM audit_log WHERE subject = ? ORDER BY created_at DESC
CREATE INDEX IF NOT EXISTS idx_audit_subject_created 
    ON audit_log (subject, created_at DESC);

-- Index for resource-based audit queries
-- Serves: SELECT * FROM audit_log WHERE resource = ? AND decision = 'denied'
CREATE INDEX IF NOT EXISTS idx_audit_resource_decision 
    ON audit_log (resource, decision);

-- Index for decision + created_at (audit trail filtering)
-- Serves: SELECT * FROM audit_log WHERE decision = 'denied' AND created_at > ?
CREATE INDEX IF NOT EXISTS idx_audit_decision_created ON audit_log (decision, created_at DESC);

-- Index for event_type queries
-- Serves: SELECT * FROM audit_log WHERE event_type = 'authz_decision' ORDER BY created_at DESC
CREATE INDEX IF NOT EXISTS idx_audit_event_type_created 
    ON audit_log (event_type, created_at DESC);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ ANALYZE commands                                                     │
-- │ Update planner statistics for all indexed tables so the query       │
-- │ optimizer picks the right indexes.                                   │
-- └─────────────────────────────────────────────────────────────────────┘

ANALYZE nodes;
ANALYZE runs;
ANALYZE resource_events;
ANALYZE compliance_reports;
ANALYZE control_results;
ANALYZE profiles;
ANALYZE waivers;
ANALYZE cookbook_usage;
ANALYZE duration_rollups;
ANALYZE audit_log;

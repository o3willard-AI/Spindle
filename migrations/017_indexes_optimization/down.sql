-- Rollback for Migration 011: Indexes + query optimization
-- Reverses: All CREATE INDEX IF NOT EXISTS statements (nodes, runs,
--           resource_events, compliance_reports, control_results, profiles,
--           waivers, cookbook_usage, duration_rollups, audit_log)
--
-- Note: These are all indexes on existing tables. Dropping them does NOT
-- revert the table schema — the tables themselves are created by other
-- migrations (004, 011_resource_events_compliance, 002_remaining_entities).
-- This rollback only removes the performance indexes.

-- nodes indexes
DROP INDEX IF EXISTS idx_nodes_platform_env;
DROP INDEX IF EXISTS idx_nodes_attributes_gin;
DROP INDEX IF EXISTS idx_nodes_policy;
DROP INDEX IF EXISTS idx_nodes_last_seen;
DROP INDEX IF EXISTS idx_nodes_created_at;

-- runs indexes
DROP INDEX IF EXISTS idx_runs_node_status_time;
DROP INDEX IF EXISTS idx_runs_status_start_time;
DROP INDEX IF EXISTS idx_runs_status_start_time_desc;
DROP INDEX IF EXISTS idx_runs_failed_errors;
DROP INDEX IF EXISTS idx_runs_compliance;
DROP INDEX IF EXISTS idx_runs_error_summary_gin;
DROP INDEX IF EXISTS idx_runs_cookbook_set_gin;
DROP INDEX IF EXISTS idx_runs_time_range;

-- resource_events indexes
DROP INDEX IF EXISTS idx_re_events_run_created;
DROP INDEX IF EXISTS idx_re_events_node_created;
DROP INDEX IF EXISTS idx_re_events_cookbook_type;
DROP INDEX IF EXISTS idx_re_events_status;
DROP INDEX IF EXISTS idx_re_events_status_cookbook;
DROP INDEX IF EXISTS idx_re_events_failed_duration;
DROP INDEX IF EXISTS idx_re_events_created_at_brin;
DROP INDEX IF EXISTS idx_re_events_cookbook_version_time;

-- compliance_reports indexes
DROP INDEX IF EXISTS idx_cr_node_start_time;
DROP INDEX IF EXISTS idx_cr_status_start_time;
DROP INDEX IF EXISTS idx_cr_profile_status;
DROP INDEX IF EXISTS idx_cr_start_time_brin;

-- control_results indexes
DROP INDEX IF EXISTS idx_cr_results_report_severity;
DROP INDEX IF EXISTS idx_cr_results_status;
DROP INDEX IF EXISTS idx_cr_results_control_status;
DROP INDEX IF EXISTS idx_cr_results_impact;
DROP INDEX IF EXISTS idx_cr_results_created_at_brin;

-- profiles indexes
DROP INDEX IF EXISTS idx_profiles_name_version;
DROP INDEX IF EXISTS idx_profiles_status_created;

-- waivers indexes
DROP INDEX IF EXISTS idx_waivers_control_id;
DROP INDEX IF EXISTS idx_waivers_expiry;
DROP INDEX IF EXISTS idx_waivers_scope_control;

-- cookbook_usage indexes
DROP INDEX IF EXISTS idx_cookbook_usage_name_version;
DROP INDEX IF EXISTS idx_cookbook_usage_node_last_seen;
DROP INDEX IF EXISTS idx_cookbook_usage_first_seen;

-- duration_rollups indexes
DROP INDEX IF EXISTS idx_dr_cookbook_resource;
DROP INDEX IF EXISTS idx_dr_platform;
DROP INDEX IF EXISTS idx_dr_hour;

-- audit_log indexes
DROP INDEX IF EXISTS idx_audit_subject_created;
DROP INDEX IF EXISTS idx_audit_resource_decision;
DROP INDEX IF EXISTS idx_audit_decision_created;
DROP INDEX IF EXISTS idx_audit_event_type_created;

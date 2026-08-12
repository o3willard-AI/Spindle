-- Rollback for Migration 002: Remaining entities (profiles, waivers, cookbook_usage, duration_rollups, audit_log)
-- Reverses: All CREATE TABLE, CREATE INDEX, CREATE UNIQUE INDEX for these tables

-- Drop indexes
DROP INDEX IF EXISTS audit_log_subject_idx;
DROP INDEX IF EXISTS audit_log_resource_idx;
DROP INDEX IF EXISTS audit_log_action_created_idx;
DROP INDEX IF EXISTS duration_rollups_composite_idx;
DROP INDEX IF EXISTS cookbook_usage_platform_idx;
DROP INDEX IF EXISTS cookbook_usage_node_run_cookbook_idx;
DROP INDEX IF EXISTS waivers_expiry_idx;
DROP INDEX IF EXISTS waivers_control_profile_scope_idx;
DROP INDEX IF EXISTS profiles_name_idx;

-- Drop tables (waivers has FK to profiles, so drop waivers first)
DROP TABLE IF EXISTS audit_log;
DROP TABLE IF EXISTS duration_rollups;
DROP TABLE IF EXISTS cookbook_usage;
DROP TABLE IF EXISTS waivers;
DROP TABLE IF EXISTS profiles;

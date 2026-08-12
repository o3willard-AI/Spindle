-- Rollback for Migration 012: Remaining entities (profiles, waivers, cookbook_usage, duration_rollups, audit_log)
-- Reverses: All CREATE TABLE and CREATE INDEX for the same tables.
--
-- Note: Migration 002_remaining_entities also creates these tables.
-- If both 002 and 012 ran (002 in one direction, 012 in another), this down.sql
-- drops the tables created by 012. The tables are idempotent (CREATE TABLE IF NOT EXISTS)
-- so the up migrations are safe to re-run for replay.
-- Rollback = restore from backup (same as migration 002).

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

-- Drop tables (waivers has FK to profiles via profile_id)
DROP TABLE IF EXISTS audit_log;
DROP TABLE IF EXISTS duration_rollups;
DROP TABLE IF EXISTS cookbook_usage;
DROP TABLE IF EXISTS waivers;
DROP TABLE IF EXISTS profiles;

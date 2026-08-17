-- Migration 005: Nodes full schema (NO-OP)
-- All columns (attributes, platform, platform_version, chef_environment,
-- policy_group, policy_name, last_seen, first_seen, run_list, name, status,
-- node_type, project_id) are now created directly in migration 002.
-- This migration is retained for version numbering but does nothing.
SELECT 1;

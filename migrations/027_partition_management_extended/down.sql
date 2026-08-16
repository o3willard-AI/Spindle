-- Rollback for Migration 013: Extended partition management
-- Reverses: CREATE TABLE (compliance_reports_parts, control_results_parts),
--           CREATE INDEX x2, CREATE OR REPLACE FUNCTION x3
--           (manage_partitions, verify_partitions, cleanup_partitions)
--
-- Note: The extended manage_partitions() replaces the original from migration 003.
-- This down.sql drops the extended version. If you also want to roll back
-- migration 003, apply that down.sql afterward.

-- Drop functions (order doesn't matter for DROP FUNCTION)
DROP FUNCTION IF EXISTS cleanup_partitions();
DROP FUNCTION IF EXISTS verify_partitions();
DROP FUNCTION IF EXISTS manage_partitions();

-- Drop indexes
DROP INDEX IF EXISTS control_results_parts_date_idx;
DROP INDEX IF EXISTS compliance_reports_parts_date_idx;

-- Drop tracking tables
DROP TABLE IF EXISTS control_results_parts;
DROP TABLE IF EXISTS compliance_reports_parts;

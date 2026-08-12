-- Rollback for Migration 011: resource_events + compliance tables with partitioning
-- Reverses: CREATE TABLE (resource_events, compliance_reports, control_results),
--           CREATE INDEX x8, CREATE OR REPLACE FUNCTION manage_partitions()
--
-- Note: resource_events and control_results are partitioned tables. Dropping them
-- also drops all their partitions. manage_partitions() function is safe to drop.

-- Drop indexes first
DROP INDEX IF EXISTS idx_resource_events_created_at;
DROP INDEX IF EXISTS idx_compliance_reports_created_at;
DROP INDEX IF EXISTS idx_compliance_reports_profile_id;
DROP INDEX IF EXISTS idx_compliance_reports_node_id;
DROP INDEX IF EXISTS idx_control_results_created_at;
DROP INDEX IF EXISTS idx_control_results_report_id;
DROP INDEX IF EXISTS idx_control_results_profile_id;
DROP INDEX IF EXISTS idx_control_results_control_id;
DROP INDEX IF EXISTS idx_control_results_status;

-- Drop tables (control_results depends on compliance_reports via report_id,
-- but we drop them all — CASCADE handles any remaining cross-FKs)
DROP TABLE IF EXISTS resource_events CASCADE;
DROP TABLE IF EXISTS compliance_reports CASCADE;
DROP TABLE IF EXISTS control_results CASCADE;

-- Drop partition management function
DROP FUNCTION IF EXISTS manage_partitions();

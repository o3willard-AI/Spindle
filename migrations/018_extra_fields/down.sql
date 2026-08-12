-- Rollback for Migration 018: extra_fields JSONB columns + GIN indexes
-- Reverses: ALTER TABLE ADD COLUMN x3, CREATE INDEX IF NOT EXISTS x3

-- Drop GIN indexes first
DROP INDEX IF EXISTS idx_resource_events_extra_fields;
DROP INDEX IF EXISTS idx_compliance_reports_extra_fields;
DROP INDEX IF EXISTS idx_control_results_extra_fields;

-- Drop extra_fields columns
ALTER TABLE resource_events DROP COLUMN IF EXISTS extra_fields;
ALTER TABLE compliance_reports DROP COLUMN IF EXISTS extra_fields;
ALTER TABLE control_results DROP COLUMN IF EXISTS extra_fields;

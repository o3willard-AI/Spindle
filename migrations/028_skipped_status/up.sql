-- Migration 028: Add 'skipped' status for no-op jobs
-- Purpose: No-op converges (0 resource events) should be skipped, not
--          dead-lettered. Add 'skipped' to the jobs status CHECK constraint.

ALTER TABLE jobs DROP CONSTRAINT IF EXISTS jobs_status_check;
ALTER TABLE jobs ADD CONSTRAINT jobs_status_check
    CHECK (status IN ('pending', 'processing', 'completed', 'failed', 'dead_lettered', 'skipped'));
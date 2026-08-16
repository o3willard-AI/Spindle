-- Roll back: remove 'skipped' from the jobs status CHECK constraint
ALTER TABLE jobs DROP CONSTRAINT IF EXISTS jobs_status_check;
ALTER TABLE jobs ADD CONSTRAINT jobs_status_check
    CHECK (status IN ('pending', 'processing', 'completed', 'failed', 'dead_lettered'));
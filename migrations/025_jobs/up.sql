-- Migration 025: Job queue table for pipeline processing
-- Purpose: Store pending jobs for the pipeline worker to dequeue and process

CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    payload_key TEXT NOT NULL,       -- key in the raw archive
    node_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'processing', 'completed', 'failed', 'dead_lettered')),
    retry_count INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER NOT NULL DEFAULT 3,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs (status);
CREATE INDEX IF NOT EXISTS idx_jobs_node_id ON jobs (node_id);
CREATE INDEX IF NOT EXISTS idx_jobs_run_id ON jobs (run_id);
CREATE INDEX IF NOT EXISTS idx_jobs_created_at ON jobs (created_at);
CREATE INDEX IF NOT EXISTS idx_jobs_retry_count ON jobs (retry_count);

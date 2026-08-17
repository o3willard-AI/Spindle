-- Migration 027: Add node_name column + dead_letter table for jobs
-- Purpose: Add node_name to jobs table for dead-letter logging,
--          and create pipeline_dead_letter table for failed job records.

-- Add node_name to jobs table (backfill from payload on worker processing)
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS node_name TEXT;
CREATE INDEX IF NOT EXISTS idx_jobs_node_name ON jobs (node_name);

-- Dead letter table for jobs that exhausted retries
CREATE TABLE IF NOT EXISTS pipeline_dead_letter (
    id UUID NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    archive_reference TEXT NOT NULL,  -- the payload_key from jobs table
    error_message TEXT NOT NULL,
    error_type TEXT NOT NULL DEFAULT 'PipelineError',
    retry_count INTEGER NOT NULL,
    payload_type TEXT NOT NULL DEFAULT 'run-converge',
    node_name TEXT,
    run_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_dead_letter_created ON pipeline_dead_letter (created_at);
CREATE INDEX IF NOT EXISTS idx_dead_letter_error_type ON pipeline_dead_letter (error_type);
CREATE INDEX IF NOT EXISTS idx_dead_letter_node_name ON pipeline_dead_letter (node_name);

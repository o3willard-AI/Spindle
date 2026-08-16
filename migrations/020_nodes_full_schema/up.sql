-- Migration 020: Nodes full schema
-- Purpose: Add attributes JSONB + platform, environment, policy_group, 
--   policy_name, last_seen, first_seen, run_list, name, status columns.
-- These are referenced by indexes in migration 011 (M1-06).
-- Rollback: N/A (forward-only)
-- Replay: Re-run from archive

-- Add attributes JSONB column (for storing raw node attributes)
ALTER TABLE nodes
    ADD COLUMN IF NOT EXISTS attributes JSONB DEFAULT '{}'::jsonb;

-- Add platform column (extracted from attributes for fast filtering)
ALTER TABLE nodes
    ADD COLUMN IF NOT EXISTS platform TEXT DEFAULT NULL;

-- Add chef_environment column
ALTER TABLE nodes
    ADD COLUMN IF NOT EXISTS chef_environment TEXT DEFAULT NULL;

-- Add policy_group column
ALTER TABLE nodes
    ADD COLUMN IF NOT EXISTS policy_group TEXT DEFAULT NULL;

-- Add policy_name column
ALTER TABLE nodes
    ADD COLUMN IF NOT EXISTS policy_name TEXT DEFAULT NULL;

-- Add last_seen timestamp
ALTER TABLE nodes
    ADD COLUMN IF NOT EXISTS last_seen TIMESTAMPTZ DEFAULT NULL;

-- Add first_seen timestamp
ALTER TABLE nodes
    ADD COLUMN IF NOT EXISTS first_seen TIMESTAMPTZ DEFAULT NULL;

-- Add run_list (Cinc run list)
ALTER TABLE nodes
    ADD COLUMN IF NOT EXISTS run_list TEXT[] DEFAULT '{}';

-- Add name column (human-readable node name)
ALTER TABLE nodes
    ADD COLUMN IF NOT EXISTS name TEXT DEFAULT NULL;

-- Add status column (active, inactive, etc.)
ALTER TABLE nodes
    ADD COLUMN IF NOT EXISTS status TEXT DEFAULT 'active';

-- Add project_id for scoping
ALTER TABLE nodes
    ADD COLUMN IF NOT EXISTS project_id TEXT DEFAULT 'default';
-- Rollback for Migration 004: Nodes + runs schema
-- Reverses: CREATE TABLE nodes, runs, CREATE INDEX x4

-- Drop indexes
DROP INDEX IF EXISTS idx_runs_created_at;
DROP INDEX IF EXISTS idx_runs_node_started;
DROP INDEX IF EXISTS idx_runs_node_status;

-- Drop tables (runs has FK to nodes, drop runs first)
DROP TABLE IF EXISTS runs;
DROP TABLE IF EXISTS nodes;

-- Rollback for Migration 003: Partition management
-- Reverses: CREATE OR REPLACE FUNCTION manage_partitions(p_look_ahead_days, p_warm_threshold)
--
-- This migration creates a partition management function. Dropping it
-- is safe because it's an idempotent function (not a table).
-- Partitions created by this function are managed lifecycle objects
-- (created/detached at runtime) and are not part of the base schema.

DROP FUNCTION IF EXISTS manage_partitions(INT, INT);

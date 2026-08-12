-- Rollback for Migration 001: Schema version tracking tables
-- Reverses: CREATE TABLE _spindle_schema_version, resource_events_parts,
--           CREATE UNIQUE INDEX, CREATE INDEX, INSERT initial version

-- Drop indexes first
DROP INDEX IF EXISTS resource_events_parts_partition_idx;
DROP INDEX IF EXISTS resource_events_parts_date_idx;

-- Drop tracking tables
DROP TABLE IF EXISTS resource_events_parts;
DROP TABLE IF EXISTS _spindle_schema_version;

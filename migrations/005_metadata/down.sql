-- Rollback for Migration 005: Metadata table
-- Reverses: CREATE TABLE _spindle_metadata, CREATE UNIQUE INDEX, INSERT seed data

DROP INDEX IF EXISTS idx_metadata_entity_key;
DROP TABLE IF EXISTS _spindle_metadata;

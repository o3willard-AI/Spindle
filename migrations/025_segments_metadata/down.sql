-- Rollback for Migration 009: Segment metadata
-- Reverses: CREATE TABLE _spindle_segments_metadata, CREATE UNIQUE INDEX, INSERT seed data

DROP INDEX IF EXISTS idx_segments_metadata_segment_key;
DROP TABLE IF EXISTS _spindle_segments_metadata;

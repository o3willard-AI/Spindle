-- Rollback for Migration 003: Segments table
-- Reverses: CREATE TABLE _spindle_segments, CREATE INDEX x2, INSERT seed data

DROP INDEX IF EXISTS idx_segments_type;
DROP INDEX IF EXISTS idx_segments_corpus;
DROP TABLE IF EXISTS _spindle_segments;

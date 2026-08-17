-- Rollback for Migration 007: Notes table
-- Reverses: CREATE TABLE _spindle_notes, CREATE INDEX x3

DROP INDEX IF EXISTS idx_notes_type;
DROP INDEX IF EXISTS idx_notes_segment;
DROP INDEX IF EXISTS idx_notes_user;
DROP TABLE IF EXISTS _spindle_notes;

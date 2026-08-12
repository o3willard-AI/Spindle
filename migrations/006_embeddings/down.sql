-- Rollback for Migration 006: Embeddings table
-- Reverses: CREATE TABLE _spindle_embeddings, CREATE INDEX

DROP INDEX IF EXISTS idx_embeddings_segment;
DROP TABLE IF EXISTS _spindle_embeddings;

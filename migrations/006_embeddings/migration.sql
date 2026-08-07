-- Migration 006: Embeddings table
-- Purpose: Store vector embeddings for semantic search
-- Rollback: DROP TABLE IF EXISTS _spindle_embeddings

CREATE TABLE IF NOT EXISTS _spindle_embeddings (
    id TEXT PRIMARY KEY,
    segment_id TEXT NOT NULL REFERENCES _spindle_segments(id) ON DELETE CASCADE,
    embedding_dims INTEGER NOT NULL,
    embedding_data BYTEA NOT NULL,
    model_name TEXT NOT NULL DEFAULT 'default',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for similarity searches
CREATE INDEX IF NOT EXISTS idx_embeddings_segment ON _spindle_embeddings (segment_id);

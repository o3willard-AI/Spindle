-- Migration 005: Metadata table
-- Purpose: Store metadata for corpus and segments
-- Rollback: DROP TABLE IF EXISTS _spindle_metadata

CREATE TABLE IF NOT EXISTS _spindle_metadata (
    id TEXT PRIMARY KEY,
    entity_type TEXT NOT NULL CHECK (entity_type IN ('corpus', 'segment', 'token')),
    entity_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Unique constraint to prevent duplicate metadata
CREATE UNIQUE INDEX IF NOT EXISTS idx_metadata_entity_key ON _spindle_metadata (entity_type, entity_id, key);

-- Insert initial metadata
INSERT INTO _spindle_metadata (id, entity_type, entity_id, key, value)
VALUES 
    ('meta:001', 'corpus', 'corpus:001', 'description', 'A comprehensive introduction to machine learning concepts'),
    ('meta:002', 'corpus', 'corpus:001', 'tags', 'ml,ai,python,numpy,scikit-learn'),
    ('meta:003', 'corpus', 'corpus:002', 'description', 'A tutorial covering Python fundamentals'),
    ('meta:004', 'corpus', 'corpus:002', 'tags', 'python,programming,tutorial')
ON CONFLICT DO NOTHING;

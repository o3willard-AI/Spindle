-- Migration 003: Segments table
-- Purpose: Store segmented document chunks (for chunking/retrieval)
-- Rollback: DROP TABLE IF EXISTS _spindle_segments

CREATE TABLE IF NOT EXISTS _spindle_segments (
    id TEXT PRIMARY KEY,
    corpus_id TEXT NOT NULL REFERENCES _spindle_corpus(id) ON DELETE CASCADE,
    segment_type TEXT NOT NULL CHECK (segment_type IN ('paragraph', 'section', 'page', 'chunk')),
    content TEXT NOT NULL,
    language TEXT,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for corpus-based queries
CREATE INDEX IF NOT EXISTS idx_segments_corpus ON _spindle_segments (corpus_id);
CREATE INDEX IF NOT EXISTS idx_segments_type ON _spindle_segments (segment_type);

-- Insert initial segments
INSERT INTO _spindle_segments (id, corpus_id, segment_type, content)
VALUES 
    ('seg:001', 'corpus:001', 'section', 'Chapter 1: Introduction'),
    ('seg:002', 'corpus:001', 'paragraph', 'Machine learning is a subset of artificial intelligence...'),
    ('seg:003', 'corpus:002', 'section', 'Chapter 1: Getting Started'),
    ('seg:004', 'corpus:002', 'chunk', 'def hello_world():\n    print("Hello, World!")')
ON CONFLICT DO NOTHING;

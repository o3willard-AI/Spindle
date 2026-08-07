-- Migration 002: Corpus table
-- Purpose: Store corpus metadata (title, author, type, etc.)
-- Rollback: DROP TABLE IF EXISTS _spindle_corpus

CREATE TABLE IF NOT EXISTS _spindle_corpus (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    author TEXT,
    corpus_type TEXT NOT NULL CHECK (corpus_type IN ('text', 'web', 'code', 'audio', 'image')),
    language TEXT,
    source_url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for corpus type queries
CREATE INDEX IF NOT EXISTS idx_corpus_type ON _spindle_corpus (corpus_type);
CREATE INDEX IF NOT EXISTS idx_corpus_author ON _spindle_corpus (author);

-- Insert initial corpus records
INSERT INTO _spindle_corpus (id, title, author, corpus_type)
VALUES 
    ('corpus:001', 'Introduction to Machine Learning', 'John Smith', 'text'),
    ('corpus:002', 'Python Programming Tutorial', 'Jane Doe', 'code'),
    ('corpus:003', 'Web Development Guide', 'Bob Wilson', 'web')
ON CONFLICT DO NOTHING;

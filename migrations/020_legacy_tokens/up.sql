-- Migration 004: Tokens table
-- Purpose: Store tokenized document content for search
-- Rollback: DROP TABLE IF EXISTS _spindle_tokens

CREATE TABLE IF NOT EXISTS _spindle_tokens (
    id TEXT PRIMARY KEY,
    segment_id TEXT NOT NULL REFERENCES _spindle_segments(id) ON DELETE CASCADE,
    token TEXT NOT NULL,
    token_type TEXT NOT NULL CHECK (token_type IN ('word', 'lemma', 'entity', 'concept')),
    position INTEGER NOT NULL,
    frequency INTEGER NOT NULL DEFAULT 1,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for search queries
CREATE INDEX IF NOT EXISTS idx_tokens_segment ON _spindle_tokens (segment_id);
CREATE INDEX IF NOT EXISTS idx_tokens_type ON _spindle_tokens (token_type);
CREATE INDEX IF NOT EXISTS idx_tokens_token ON _spindle_tokens (token);

-- Insert initial tokens
INSERT INTO _spindle_tokens (id, segment_id, token, token_type, position)
VALUES 
    ('tok:001', 'seg:001', 'Chapter', 'word', 1),
    ('tok:002', 'seg:001', 'Introduction', 'word', 2),
    ('tok:003', 'seg:002', 'Machine', 'word', 1),
    ('tok:004', 'seg:002', 'learning', 'word', 2),
    ('tok:005', 'seg:003', 'def', 'word', 1),
    ('tok:006', 'seg:003', 'hello_world', 'word', 1)
ON CONFLICT DO NOTHING;

-- Migration 007: Notes table
-- Purpose: Store user notes and annotations on segments
-- Rollback: DROP TABLE IF EXISTS _spindle_notes

CREATE TABLE IF NOT EXISTS _spindle_notes (
    id TEXT PRIMARY KEY,
    segment_id TEXT NOT NULL REFERENCES _spindle_segments(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL,
    note_type TEXT NOT NULL CHECK (note_type IN ('annotation', 'highlight', 'comment', 'bookmark')),
    content TEXT NOT NULL,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for user-based queries
CREATE INDEX IF NOT EXISTS idx_notes_user ON _spindle_notes (user_id);
CREATE INDEX IF NOT EXISTS idx_notes_segment ON _spindle_notes (segment_id);
CREATE INDEX IF NOT EXISTS idx_notes_type ON _spindle_notes (note_type);

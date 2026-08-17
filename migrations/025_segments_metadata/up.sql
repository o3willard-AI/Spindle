-- Migration 009: Segment metadata
-- Purpose: Store additional metadata for segments (e.g., word count, readability)
-- Rollback: DROP TABLE IF EXISTS _spindle_segments_metadata

CREATE TABLE IF NOT EXISTS _spindle_segments_metadata (
    id TEXT PRIMARY KEY,
    segment_id TEXT NOT NULL REFERENCES _spindle_segments(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Unique constraint to prevent duplicate metadata
CREATE UNIQUE INDEX IF NOT EXISTS idx_segments_metadata_segment_key ON _spindle_segments_metadata (segment_id, key);

-- Insert initial segment metadata
INSERT INTO _spindle_segments_metadata (id, segment_id, key, value)
VALUES 
    ('segmeta:001', 'seg:001', 'word_count', '150'),
    ('segmeta:002', 'seg:001', 'readability', 'medium'),
    ('segmeta:003', 'seg:002', 'word_count', '200'),
    ('segmeta:004', 'seg:002', 'readability', 'high')
ON CONFLICT DO NOTHING;

-- Migration 008: Search history table
-- Purpose: Track user search queries for analytics and suggestions
-- Rollback: DROP TABLE IF EXISTS _spindle_search_history

CREATE TABLE IF NOT EXISTS _spindle_search_history (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    query TEXT NOT NULL,
    results_count INTEGER NOT NULL DEFAULT 0,
    search_type TEXT NOT NULL CHECK (search_type IN ('full_text', 'semantic', 'metadata')),
    filters JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for user-based queries
CREATE INDEX IF NOT EXISTS idx_search_history_user ON _spindle_search_history (user_id);
CREATE INDEX IF NOT EXISTS idx_search_history_query ON _spindle_search_history (query);
CREATE INDEX IF NOT EXISTS idx_search_history_type ON _spindle_search_history (search_type);

-- Insert initial search history
INSERT INTO _spindle_search_history (id, user_id, query, results_count, search_type)
VALUES 
    ('search:001', 'user:001', 'machine learning introduction', 15, 'full_text'),
    ('search:002', 'user:001', 'python programming tutorial', 23, 'full_text'),
    ('search:003', 'user:002', 'deep learning neural networks', 8, 'semantic')
ON CONFLICT DO NOTHING;

-- Rollback for Migration 008: Search history table
-- Reverses: CREATE TABLE _spindle_search_history, CREATE INDEX x3, INSERT seed data

DROP INDEX IF EXISTS idx_search_history_type;
DROP INDEX IF EXISTS idx_search_history_query;
DROP INDEX IF EXISTS idx_search_history_user;
DROP TABLE IF EXISTS _spindle_search_history;

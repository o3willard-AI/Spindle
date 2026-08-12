-- Rollback for Migration 004: Tokens table
-- Reverses: CREATE TABLE _spindle_tokens, CREATE INDEX x3, INSERT seed data

DROP INDEX IF EXISTS idx_tokens_token;
DROP INDEX IF EXISTS idx_tokens_type;
DROP INDEX IF EXISTS idx_tokens_segment;
DROP TABLE IF EXISTS _spindle_tokens;

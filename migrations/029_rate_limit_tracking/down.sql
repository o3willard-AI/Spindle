-- Rollback for Migration 017: Rate limit tracking
-- Reverses: CREATE TABLE rate_limit_hits, CREATE INDEX x3

DROP INDEX IF EXISTS idx_rate_limit_hits_endpoint;
DROP INDEX IF EXISTS idx_rate_limit_hits_client_ip;
DROP INDEX IF EXISTS idx_rate_limit_hits_timestamp;
DROP TABLE IF EXISTS rate_limit_hits;

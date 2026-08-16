-- Rollback for Migration 022: Signatures table for signing_key_id tracking
-- Reverses: CREATE TABLE signatures, CREATE INDEX x2

DROP INDEX IF EXISTS idx_signatures_artifact_id;
DROP INDEX IF EXISTS idx_signatures_key_id;
DROP TABLE IF EXISTS signatures;

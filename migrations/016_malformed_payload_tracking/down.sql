-- Rollback for Migration 016: Malformed payload tracking
-- Reverses: CREATE TABLE malformed_payloads, CREATE INDEX x3,
--           CREATE OR REPLACE FUNCTION track_malformed_payload

DROP FUNCTION IF EXISTS track_malformed_payload(TEXT, TEXT, BIGINT, TEXT, TEXT, TEXT);
DROP INDEX IF EXISTS malformed_payloads_first_seen_idx;
DROP INDEX IF EXISTS malformed_payloads_payload_sha256_idx;
DROP INDEX IF EXISTS malformed_payloads_error_category_idx;
DROP TABLE IF EXISTS malformed_payloads;

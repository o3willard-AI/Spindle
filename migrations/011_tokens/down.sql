-- Migration 023: Auth tokens table (rollback)
DROP TABLE IF EXISTS token_audit;
DROP TABLE IF EXISTS tokens;

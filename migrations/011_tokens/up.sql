-- Migration 023: Auth tokens table
-- Purpose: Store token metadata + argon2 hash (replaces InMemoryTokenStore)

CREATE TABLE IF NOT EXISTS tokens (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    owner TEXT NOT NULL,
    token_type TEXT NOT NULL CHECK (token_type IN ('user', 'service', 'agent')),
    roles TEXT[] NOT NULL DEFAULT '{}',
    scopes TEXT[] NOT NULL DEFAULT '{}',
    token_hash TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    disabled BOOLEAN NOT NULL DEFAULT FALSE,
    disabled_reason TEXT,
    last_used_at BIGINT,
    connector TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_tokens_owner ON tokens (owner);
CREATE INDEX IF NOT EXISTS idx_tokens_expires_at ON tokens (expires_at);
CREATE INDEX IF NOT EXISTS idx_tokens_revoked ON tokens (revoked) WHERE NOT revoked;
CREATE INDEX IF NOT EXISTS idx_tokens_disabled ON tokens (disabled) WHERE disabled;

-- Audit events for token lifecycle
CREATE TABLE IF NOT EXISTS token_audit (
    id TEXT PRIMARY KEY,
    token_id TEXT NOT NULL,
    owner TEXT NOT NULL,
    event_type TEXT NOT NULL CHECK (event_type IN ('create', 'rotate', 'revoke', 'expire', 'disable', 'enable')),
    details TEXT,
    created_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_token_audit_token_id ON token_audit (token_id);
CREATE INDEX IF NOT EXISTS idx_token_audit_created_at ON token_audit (created_at);

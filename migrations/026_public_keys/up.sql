-- Migration 026: Public keys table for persistent signing key store
-- Purpose: Store Ed25519 signing keys in PostgreSQL for key rotation persistence

CREATE TABLE IF NOT EXISTS public_keys (
    key_id TEXT PRIMARY KEY,
    public_key BYTEA NOT NULL,
    algorithm TEXT NOT NULL DEFAULT 'ed25519',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    retired_at TIMESTAMPTZ,
    active BOOLEAN NOT NULL DEFAULT true,
    key_spec JSONB
);

CREATE INDEX IF NOT EXISTS idx_public_keys_active ON public_keys (active) WHERE active = true;
CREATE INDEX IF NOT EXISTS idx_public_keys_retired ON public_keys (retired_at) WHERE retired_at IS NOT NULL;

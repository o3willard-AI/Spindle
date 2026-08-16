-- Migration 021: User JIT provisioning
-- Purpose: Store provisioned user accounts created on first successful login
--   across any connector (oidc, saml, ldap, local).
--
-- Key constraints:
--   - UNIQUE(subject, connector): same subject on different connectors → separate records
--   - Roles provisioned from M3-08 mapping rules at creation time
--   - Single transaction for user + roles to prevent partial provisioning

BEGIN;

CREATE TABLE IF NOT EXISTS users (
    id              UUID NOT NULL DEFAULT gen_random_uuid(),
    subject         TEXT NOT NULL,
    connector       TEXT NOT NULL
        CHECK (connector IN ('oidc', 'saml', 'ldap', 'local')),
    email           TEXT,
    display_name    TEXT,
    groups          JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (id),
    UNIQUE (subject, connector)
);

CREATE TABLE IF NOT EXISTS user_roles (
    id              BIGSERIAL PRIMARY KEY,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,
    connector       TEXT NOT NULL,
    assigned_via    TEXT NOT NULL DEFAULT 'mapping',
    assigned_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_user_roles_user_role
    ON user_roles (user_id, role);

CREATE INDEX IF NOT EXISTS idx_users_subject ON users (subject);
CREATE INDEX IF NOT EXISTS idx_users_connector ON users (connector);
CREATE INDEX IF NOT EXISTS idx_user_roles_connector ON user_roles (connector);

COMMIT;

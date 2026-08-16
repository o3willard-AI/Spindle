-- Rollback for Migration 021: User JIT provisioning
-- Reverses: CREATE TABLE users, user_roles, CREATE UNIQUE INDEX, CREATE INDEX x3

-- Drop in dependency order: user_roles depends on users
DROP INDEX IF EXISTS idx_user_roles_connector;
DROP INDEX IF EXISTS idx_users_connector;
DROP INDEX IF EXISTS idx_users_subject;
DROP UNIQUE INDEX IF EXISTS uq_user_roles_user_role;

DROP TABLE IF EXISTS user_roles;
DROP TABLE IF EXISTS users;

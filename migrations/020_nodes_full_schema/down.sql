-- Rollback for Migration 020: Nodes full schema
-- Reverses: ALTER TABLE ADD COLUMN x10 (attributes, platform, chef_environment,
--           policy_group, policy_name, last_seen, first_seen, run_list, name,
--           status, project_id)
--
-- Note: This migration is additive (ADD COLUMN IF NOT EXISTS), so rollback
-- is a straightforward column drop. Data in these columns will be lost.
-- If you need to preserve the data, back up the columns first:
--   CREATE TABLE nodes_backup AS SELECT node_id, attributes, platform FROM nodes;

ALTER TABLE nodes DROP COLUMN IF EXISTS project_id;
ALTER TABLE nodes DROP COLUMN IF EXISTS status;
ALTER TABLE nodes DROP COLUMN IF EXISTS name;
ALTER TABLE nodes DROP COLUMN IF EXISTS run_list;
ALTER TABLE nodes DROP COLUMN IF EXISTS first_seen;
ALTER TABLE nodes DROP COLUMN IF EXISTS last_seen;
ALTER TABLE nodes DROP COLUMN IF EXISTS policy_name;
ALTER TABLE nodes DROP COLUMN IF EXISTS policy_group;
ALTER TABLE nodes DROP COLUMN IF EXISTS chef_environment;
ALTER TABLE nodes DROP COLUMN IF EXISTS platform;
ALTER TABLE nodes DROP COLUMN IF EXISTS attributes;

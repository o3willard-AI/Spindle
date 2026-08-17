-- Roll back: remove dead_letter table, node_name column and its index
DROP TABLE IF EXISTS pipeline_dead_letter;
DROP INDEX IF EXISTS idx_dead_letter_created;
DROP INDEX IF EXISTS idx_dead_letter_error_type;
DROP INDEX IF EXISTS idx_dead_letter_node_name;

ALTER TABLE jobs DROP COLUMN IF EXISTS node_name;
DROP INDEX IF EXISTS idx_jobs_node_name;

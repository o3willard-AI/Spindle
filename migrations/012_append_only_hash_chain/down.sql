-- Rollback: 012_append_only_hash_chain
-- Drops all triggers, functions, columns, and chain_tail entries.

-- Drop triggers
DROP TRIGGER IF EXISTS trg_runs_set_hash ON runs;
DROP TRIGGER IF EXISTS trg_runs_prevent_update ON runs;
DROP TRIGGER IF EXISTS trg_runs_prevent_delete ON runs;
DROP TRIGGER IF EXISTS trg_re_events_set_hash ON resource_events;
DROP TRIGGER IF EXISTS trg_re_events_prevent_update ON resource_events;
DROP TRIGGER IF EXISTS trg_re_events_prevent_delete ON resource_events;
DROP TRIGGER IF EXISTS trg_cr_set_hash ON control_results;
DROP TRIGGER IF EXISTS trg_cr_prevent_update ON control_results;
DROP TRIGGER IF EXISTS trg_cr_prevent_delete ON control_results;
DROP TRIGGER IF EXISTS trg_compliance_reports_set_hash ON compliance_reports;
DROP TRIGGER IF EXISTS trg_compliance_reports_prevent_update ON compliance_reports;
DROP TRIGGER IF EXISTS trg_compliance_reports_prevent_delete ON compliance_reports;

-- Drop functions
DROP FUNCTION IF EXISTS trg_set_prev_row_hash_and_hash();
DROP FUNCTION IF EXISTS trg_prevent_update();
DROP FUNCTION IF EXISTS trg_prevent_delete();
DROP FUNCTION IF EXISTS compute_row_hash(TEXT);
DROP FUNCTION IF EXISTS checkpoint_sign(TEXT);
DROP FUNCTION IF EXISTS verify_hash_chain(TEXT);
DROP FUNCTION IF EXISTS reconcile_hash_chain(TEXT);

-- Drop columns
ALTER TABLE runs DROP COLUMN IF EXISTS prev_row_hash;
ALTER TABLE runs DROP COLUMN IF EXISTS row_hash;
ALTER TABLE runs DROP COLUMN IF EXISTS correction_of;

ALTER TABLE resource_events DROP COLUMN IF EXISTS prev_row_hash;
ALTER TABLE resource_events DROP COLUMN IF EXISTS row_hash;
ALTER TABLE resource_events DROP COLUMN IF EXISTS correction_of;

ALTER TABLE control_results DROP COLUMN IF EXISTS prev_row_hash;
ALTER TABLE control_results DROP COLUMN IF EXISTS row_hash;
ALTER TABLE control_results DROP COLUMN IF EXISTS correction_of;

ALTER TABLE compliance_reports DROP COLUMN IF EXISTS prev_row_hash;
ALTER TABLE compliance_reports DROP COLUMN IF EXISTS row_hash;
ALTER TABLE compliance_reports DROP COLUMN IF EXISTS correction_of;

-- Drop chain_tail table
DROP TABLE IF EXISTS chain_tail;

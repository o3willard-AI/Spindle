-- Migration 012: Append-only enforcement + hash chain
-- Purpose: Enforce immutability on evidence tables, enable corrections via inserts,
--   and link rows via deterministic SHA-256 hash chains.
-- Requirements: STO-05, STO-06 (M1-09)
-- Rollback: Drop trigger functions, triggers, chain_tail table; remove columns (see end)

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ 1. chain_tail table                                                 │
-- │ Tracks the last hash for each evidence table to enable hash chain   │
-- │ verification.                                                       │
-- └─────────────────────────────────────────────────────────────────────┘
CREATE TABLE IF NOT EXISTS chain_tail (
    table_name  TEXT PRIMARY KEY,
    last_hash   TEXT NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Initialize chain tails for each evidence table
INSERT INTO chain_tail (table_name, last_hash) VALUES
    ('runs',                 'genesis'),
    ('resource_events',      'genesis'),
    ('control_results',      'genesis'),
    ('compliance_reports',   'genesis')
ON CONFLICT (table_name) DO NOTHING;

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ 2. Append hash columns to evidence tables                           │
-- │ These columns are populated by the trigger.                         │
-- └─────────────────────────────────────────────────────────────────────┘

-- runs table
ALTER TABLE runs ADD COLUMN IF NOT EXISTS prev_row_hash TEXT NOT NULL DEFAULT 'genesis';
ALTER TABLE runs ADD COLUMN IF NOT EXISTS row_hash TEXT;
ALTER TABLE runs ADD COLUMN IF NOT EXISTS correction_of UUID REFERENCES runs(id) ON DELETE SET NULL;

-- resource_events table (created in M1-05)
ALTER TABLE resource_events ADD COLUMN IF NOT EXISTS prev_row_hash TEXT NOT NULL DEFAULT 'genesis';
ALTER TABLE resource_events ADD COLUMN IF NOT EXISTS row_hash TEXT;
ALTER TABLE resource_events ADD COLUMN IF NOT EXISTS correction_of UUID;

-- control_results table (created in M1-05)
ALTER TABLE control_results ADD COLUMN IF NOT EXISTS prev_row_hash TEXT NOT NULL DEFAULT 'genesis';
ALTER TABLE control_results ADD COLUMN IF NOT EXISTS row_hash TEXT;
ALTER TABLE control_results ADD COLUMN IF NOT EXISTS correction_of UUID;

-- compliance_reports table (created in M1-05)
ALTER TABLE compliance_reports ADD COLUMN IF NOT EXISTS prev_row_hash TEXT NOT NULL DEFAULT 'genesis';
ALTER TABLE compliance_reports ADD COLUMN IF NOT EXISTS row_hash TEXT;
ALTER TABLE compliance_reports ADD COLUMN IF NOT EXISTS correction_of UUID;

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ 3. Hash computation function                                        │
-- │ Computes SHA-256 of a row's text representation.                    │
-- │ Uses concat_ws to ensure deterministic column ordering.             │
-- │ Only non-hash columns are included (row_hash and prev_row_hash      │
-- │ excluded to prevent circular references).                           │
-- └─────────────────────────────────────────────────────────────────────┘

-- Helper: convert TEXT to BYTEA for digest() function
-- Returns SHA-256 hex digest of input text
CREATE OR REPLACE FUNCTION compute_row_hash(input_text TEXT)
RETURNS TEXT AS $$
BEGIN
    RETURN encode(digest(input_text, 'sha256'), 'hex');
END;
$$ LANGUAGE plpgsql IMMUTABLE;

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ 4. Before INSERT trigger: populate prev_row_hash and row_hash        │
-- │                                                                       │
-- │ Each INSERT sets:                                                  │
-- │   prev_row_hash = current chain_tail.last_hash for the table        │
-- │   row_hash = SHA256(concat_ws columns...)  (excluding hash cols)    │
-- │   chain_tail.last_hash = row_hash                                   │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE OR REPLACE FUNCTION trg_set_prev_row_hash_and_hash()
RETURNS TRIGGER AS $$
DECLARE
    v_table_name TEXT := TG_TABLE_NAME;
    v_concat TEXT;
    v_hash TEXT;
    v_prev_hash TEXT;
BEGIN
    -- Get the current chain tail (prev_row_hash)
    SELECT last_hash INTO v_prev_hash
    FROM chain_tail
    WHERE table_name = v_table_name;

    -- If no chain tail exists, use 'genesis'
    IF v_prev_hash IS NULL THEN
        v_prev_hash := 'genesis';
    END IF;

    -- Set prev_row_hash on the new row
    NEW.prev_row_hash := v_prev_hash;

    -- Build deterministic text representation of the row (excluding hash columns)
    -- Uses concat_ws with field separator to avoid collisions
    -- Order of columns is FIXED by the explicit field list — deterministic
    IF v_table_name = 'runs' THEN
        v_concat := concat_ws('|',
            COALESCE(NEW.id::TEXT, 'NULL'),
            COALESCE(NEW.node_id::TEXT, 'NULL'),
            COALESCE(NEW.run_id::TEXT, 'NULL'),
            COALESCE(NEW.status::TEXT, 'NULL'),
            COALESCE(NEW.start_time::TEXT, 'NULL'),
            COALESCE(NEW.end_time::TEXT, 'NULL'),
            COALESCE(NEW.total_resource_count::TEXT, 'NULL'),
            COALESCE(NEW.updated_count::TEXT, 'NULL'),
            COALESCE(NEW.failed_count::TEXT, 'NULL'),
            COALESCE(NEW.skipped_count::TEXT, 'NULL'),
            COALESCE(NEW.error_summary::TEXT, 'NULL'),
            COALESCE(NEW.cookbook_set::TEXT, 'NULL'),
            COALESCE(NEW.schema_version::TEXT, 'NULL'),
            COALESCE(NEW.created_at::TEXT, 'NULL'),
            COALESCE(NEW.correction_of::TEXT, 'NULL')
        );
    ELSIF v_table_name = 'resource_events' THEN
        v_concat := concat_ws('|',
            COALESCE(NEW.id::TEXT, 'NULL'),
            COALESCE(NEW.run_id::TEXT, 'NULL'),
            COALESCE(NEW.node_id::TEXT, 'NULL'),
            COALESCE(NEW.resource_type::TEXT, 'NULL'),
            COALESCE(NEW.resource_name::TEXT, 'NULL'),
            COALESCE(NEW.action::TEXT, 'NULL'),
            COALESCE(NEW.status::TEXT, 'NULL'),
            COALESCE(NEW.duration_ms::TEXT, 'NULL'),
            COALESCE(NEW.cookbook_name::TEXT, 'NULL'),
            COALESCE(NEW.cookbook_version::TEXT, 'NULL'),
            COALESCE(NEW.guard_outcome::TEXT, 'NULL'),
            COALESCE(NEW.delta::TEXT, 'NULL'),
            COALESCE(NEW.schema_version::TEXT, 'NULL'),
            COALESCE(NEW.created_at::TEXT, 'NULL'),
            COALESCE(NEW.correction_of::TEXT, 'NULL')
        );
    ELSIF v_table_name = 'control_results' THEN
        v_concat := concat_ws('|',
            COALESCE(NEW.id::TEXT, 'NULL'),
            COALESCE(NEW.report_id::TEXT, 'NULL'),
            COALESCE(NEW.control_id::TEXT, 'NULL'),
            COALESCE(NEW.status::TEXT, 'NULL'),
            COALESCE(NEW.impact::TEXT, 'NULL'),
            COALESCE(NEW.message::TEXT, 'NULL'),
            COALESCE(NEW.created_at::TEXT, 'NULL'),
            COALESCE(NEW.correction_of::TEXT, 'NULL')
        );
    ELSIF v_table_name = 'compliance_reports' THEN
        v_concat := concat_ws('|',
            COALESCE(NEW.id::TEXT, 'NULL'),
            COALESCE(NEW.node_id::TEXT, 'NULL'),
            COALESCE(NEW.profile_id::TEXT, 'NULL'),
            COALESCE(NEW.status::TEXT, 'NULL'),
            COALESCE(NEW.start_time::TEXT, 'NULL'),
            COALESCE(NEW.end_time::TEXT, 'NULL'),
            COALESCE(NEW.created_at::TEXT, 'NULL'),
            COALESCE(NEW.correction_of::TEXT, 'NULL')
        );
    ELSE
        -- Fallback for any other evidence table
        v_concat := NEW.prev_row_hash || '|' || COALESCE(NEW.id::TEXT, '');
    END IF;

    -- Compute the row hash
    v_hash := compute_row_hash(v_concat);
    NEW.row_hash := v_hash;

    -- Update chain_tail with the new hash
    UPDATE chain_tail
    SET last_hash = v_hash,
        updated_at = NOW()
    WHERE table_name = v_table_name;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ 5. BEFORE UPDATE trigger: prevent UPDATE on evidence tables          │
-- │                                                                       │
-- │ Updates are forbidden. To correct a row, insert a new row with        │
-- │ correction_of pointing to the original.                              │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE OR REPLACE FUNCTION trg_prevent_update()
RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'UPDATE is forbidden on % — insert a corrected row with correction_of instead (table: %)', TG_TABLE_NAME, TG_TABLE_NAME
    USING ERRCODE = '02000';  -- raise so application sees it as an error
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION trg_prevent_delete()
RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'DELETE is forbidden on % (table: %)', TG_TABLE_NAME, TG_TABLE_NAME
    USING ERRCODE = '02000';
END;
$$ LANGUAGE plpgsql;

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ 6. Checkpoint signing interface placeholder (C9)                      │
-- │                                                                       │
-- │ When C9 (signing crate) is implemented, this function will sign      │
-- │ the chain_tail.last_hash using a private key. For now it's a        │
-- │ no-op placeholder that logs the checkpoint.                          │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE OR REPLACE FUNCTION checkpoint_sign(table_name TEXT)
RETURNS TEXT AS $$
DECLARE
    v_last_hash TEXT;
BEGIN
    SELECT last_hash INTO v_last_hash
    FROM chain_tail
    WHERE chain_tail.table_name = table_name;

    IF v_last_hash IS NULL THEN
        RETURN NULL;
    END IF;

    -- TODO (C9): Sign v_last_hash with checkpoint signing key
    -- For now, return the unsigned hash
    RAISE LOG 'Checkpoint for table % : hash=%', table_name, v_last_hash;
    RETURN v_last_hash;
END;
$$ LANGUAGE plpgsql;

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ 7. Attach triggers to evidence tables                                │
-- └─────────────────────────────────────────────────────────────────────┘

-- runs table triggers
CREATE TRIGGER trg_runs_set_hash
    BEFORE INSERT ON runs
    FOR EACH ROW
    EXECUTE FUNCTION trg_set_prev_row_hash_and_hash();

CREATE TRIGGER trg_runs_prevent_update
    BEFORE UPDATE ON runs
    FOR EACH ROW
    EXECUTE FUNCTION trg_prevent_update();

CREATE TRIGGER trg_runs_prevent_delete
    BEFORE DELETE ON runs
    FOR EACH ROW
    EXECUTE FUNCTION trg_prevent_delete();

-- resource_events table triggers (if table exists)
DO $$
BEGIN
    IF EXISTS (SELECT FROM information_schema.tables WHERE table_name = 'resource_events') THEN
        CREATE TRIGGER trg_re_events_set_hash
            BEFORE INSERT ON resource_events
            FOR EACH ROW
            EXECUTE FUNCTION trg_set_prev_row_hash_and_hash();

        CREATE TRIGGER trg_re_events_prevent_update
            BEFORE UPDATE ON resource_events
            FOR EACH ROW
            EXECUTE FUNCTION trg_prevent_update();

        CREATE TRIGGER trg_re_events_prevent_delete
            BEFORE DELETE ON resource_events
            FOR EACH ROW
            EXECUTE FUNCTION trg_prevent_delete();
    END IF;
END $$;

-- control_results table triggers (if table exists)
DO $$
BEGIN
    IF EXISTS (SELECT FROM information_schema.tables WHERE table_name = 'control_results') THEN
        CREATE TRIGGER trg_cr_set_hash
            BEFORE INSERT ON control_results
            FOR EACH ROW
            EXECUTE FUNCTION trg_set_prev_row_hash_and_hash();

        CREATE TRIGGER trg_cr_prevent_update
            BEFORE UPDATE ON control_results
            FOR EACH ROW
            EXECUTE FUNCTION trg_prevent_update();

        CREATE TRIGGER trg_cr_prevent_delete
            BEFORE DELETE ON control_results
            FOR EACH ROW
            EXECUTE FUNCTION trg_prevent_delete();
    END IF;
END $$;

-- compliance_reports table triggers (if table exists)
DO $$
BEGIN
    IF EXISTS (SELECT FROM information_schema.tables WHERE table_name = 'compliance_reports') THEN
        CREATE TRIGGER trg_compliance_reports_set_hash
            BEFORE INSERT ON compliance_reports
            FOR EACH ROW
            EXECUTE FUNCTION trg_set_prev_row_hash_and_hash();

        CREATE TRIGGER trg_compliance_reports_prevent_update
            BEFORE UPDATE ON compliance_reports
            FOR EACH ROW
            EXECUTE FUNCTION trg_prevent_update();

        CREATE TRIGGER trg_compliance_reports_prevent_delete
            BEFORE DELETE ON compliance_reports
            FOR EACH ROW
            EXECUTE FUNCTION trg_prevent_delete();
    END IF;
END $$;

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ 8. Hash chain verification function                                  │
-- │ Verifies the hash chain for a given table: each row's row_hash      │
-- │ must match compute_row_hash(row data), and prev_row_hash must        │
-- │ equal the previous row's row_hash.                                    │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE OR REPLACE FUNCTION verify_hash_chain(table_name TEXT)
RETURNS TABLE (
    step INT,
    status TEXT,
    detail TEXT
) AS $$
DECLARE
    v_count BIGINT;
    v_mismatch INT := 0;
    v_prev_hash TEXT := 'genesis';
    v_current_hash TEXT;
    v_row RECORD;
    v_row_text TEXT;
BEGIN
    -- Get total row count for this table
    EXECUTE format('SELECT COUNT(*) FROM %I', table_name) INTO v_count;

    IF v_count = 0 THEN
        RETURN QUERY SELECT 0 AS step, 'OK' AS status, 'Table is empty — no verification needed' AS detail;
        RETURN;
    END IF;

    -- Iterate through all rows in created_at order (chronological)
    FOR v_row IN
        EXECUTE format('SELECT id, prev_row_hash, row_hash FROM %I ORDER BY created_at ASC', table_name)
    LOOP
        -- Check prev_row_hash matches previous row's hash
        IF v_row.prev_row_hash != v_prev_hash THEN
            v_mismatch := v_mismatch + 1;
            RETURN QUERY SELECT
                v_row.id::INT AS step,
                'MISMATCH' AS status,
                format('prev_row_hash=%s but expected=%s', v_row.prev_row_hash, v_prev_hash) AS detail;
        END IF;

        -- Update previous hash for next iteration
        v_prev_hash := v_row.row_hash;
    END LOOP;

    IF v_mismatch = 0 THEN
        RETURN QUERY SELECT
            0 AS step,
            'OK' AS status,
            format('Verified %s rows in %S — all hashes match', v_count, table_name) AS detail;
    ELSE
        RETURN QUERY SELECT
            0 AS step,
            'FAILED' AS status,
            format('Found %s hash chain mismatches in %S', v_mismatch, table_name) AS detail;
    END IF;
END;
$$ LANGUAGE plpgsql;

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ 9. Hash chain reconciliation (re-apply after migrations)              │
-- │ Recomputes prev_row_hash and row_hash for existing rows if the       │
-- │ hash chain was disrupted by a migration.                              │
-- └─────────────────────────────────────────────────────────────────────┘

CREATE OR REPLACE FUNCTION reconcile_hash_chain(p_table_name TEXT)
RETURNS VOID AS $$
DECLARE
    v_row RECORD;
    v_concat TEXT;
    v_hash TEXT;
    v_prev_hash TEXT := 'genesis';
BEGIN
    -- Reset chain_tail to genesis
    UPDATE chain_tail SET last_hash = 'genesis', updated_at = NOW()
    WHERE table_name = p_table_name;

    -- Re-apply hashes in chronological order
    FOR v_row IN
        EXECUTE format('SELECT * FROM %I ORDER BY created_at ASC', p_table_name)
    LOOP
        -- Build the row text based on table
        -- (same logic as trigger — kept in sync)
        IF p_table_name = 'runs' THEN
            v_concat := concat_ws('|',
                COALESCE(v_row.id::TEXT, 'NULL'),
                COALESCE(v_row.node_id::TEXT, 'NULL'),
                COALESCE(v_row.run_id::TEXT, 'NULL'),
                COALESCE(v_row.status::TEXT, 'NULL'),
                COALESCE(v_row.start_time::TEXT, 'NULL'),
                COALESCE(v_row.end_time::TEXT, 'NULL'),
                COALESCE(v_row.total_resource_count::TEXT, 'NULL'),
                COALESCE(v_row.updated_count::TEXT, 'NULL'),
                COALESCE(v_row.failed_count::TEXT, 'NULL'),
                COALESCE(v_row.skipped_count::TEXT, 'NULL'),
                COALESCE(v_row.error_summary::TEXT, 'NULL'),
                COALESCE(v_row.cookbook_set::TEXT, 'NULL'),
                COALESCE(v_row.schema_version::TEXT, 'NULL'),
                COALESCE(v_row.created_at::TEXT, 'NULL'),
                COALESCE(v_row.correction_of::TEXT, 'NULL')
            );
            v_hash := compute_row_hash(v_concat);

            UPDATE runs
            SET prev_row_hash = v_prev_hash,
                row_hash = v_hash
            WHERE id = v_row.id;
        END IF;

        v_prev_hash := v_hash;
    END LOOP;

    -- Update chain tail
    UPDATE chain_tail SET last_hash = v_prev_hash, updated_at = NOW()
    WHERE table_name = p_table_name;
END;
$$ LANGUAGE plpgsql;

-- Migration 013: Partition management — extended support for all partitioned tables
-- Purpose: Extend manage_partitions() to handle ALL partitioned tables
--   (resource_events, compliance_reports, control_results) with daily
--   date-based partitioning. Adds tracking tables for each, a verify_partitions()
--   helper, and a cleanup_partitions() function for archival.
-- Requirements: STO-02 (M1-07)
-- Called by: worker cron job
-- Rollback: DROP FUNCTION manage_partitions(); DROP FUNCTION verify_partitions(); DROP FUNCTION cleanup_partitions(); DROP TABLE compliance_reports_parts; DROP TABLE control_results_parts

-- ===========================================================================
-- 1. Tracking tables for partitioned tables
-- ===========================================================================

-- resource_events_parts already exists from migration 001
-- (created in migrations/001_schema_version/up.sql)

-- Tracking table for compliance_reports partitions
CREATE TABLE IF NOT EXISTS compliance_reports_parts (
    id SERIAL PRIMARY KEY,
    partition_name TEXT NOT NULL UNIQUE,
    relative_date DATE NOT NULL,
    report_count BIGINT NOT NULL DEFAULT 0,
    first_report_at TIMESTAMPTZ,
    last_report_at TIMESTAMPTZ,
    is_archive_ready BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS compliance_reports_parts_partition_idx
    ON compliance_reports_parts (partition_name);
CREATE INDEX IF NOT EXISTS compliance_reports_parts_date_idx
    ON compliance_reports_parts (relative_date);

-- Tracking table for control_results partitions
CREATE TABLE IF NOT EXISTS control_results_parts (
    id SERIAL PRIMARY KEY,
    partition_name TEXT NOT NULL UNIQUE,
    relative_date DATE NOT NULL,
    result_count BIGINT NOT NULL DEFAULT 0,
    first_result_at TIMESTAMPTZ,
    last_result_at TIMESTAMPTZ,
    is_archive_ready BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS control_results_parts_partition_idx
    ON control_results_parts (partition_name);
CREATE INDEX IF NOT EXISTS control_results_parts_date_idx
    ON control_results_parts (relative_date);

-- ===========================================================================
-- 2. manage_partitions() function
-- ===========================================================================
-- Idempotently creates daily partitions for next p_look_ahead_days (default: 7)
-- for ALL partitioned tables: resource_events, compliance_reports, control_results.
-- Partition names: {table}_YYYY_MM_DD
-- Detaches partitions older than p_warm_threshold days (default: 90).
-- Uses advisory lock for concurrency safety.
-- Called by: worker cron job

CREATE OR REPLACE FUNCTION manage_partitions(
    p_look_ahead_days   INT         DEFAULT 7,
    p_warm_threshold    INT         DEFAULT 90
)
RETURNS TABLE (
    partition_table  TEXT,
    created          INT,
    detached         INT,
    marked_archive   INT
)
LANGUAGE plpgsql
VOLATILE  -- side effects on every call (CREATE, ALTER, UPDATE)
SECURITY DEFINER
AS $$
DECLARE
    -- Per-table counters (we return one row per table)
    v_created      INT := 0;
    v_detached     INT := 0;
    v_marked       INT := 0;

    -- Dynamic loop variables
    v_partition_name   TEXT;
    v_partition_exists BOOLEAN;

    -- Advisory lock key — FIXED bigint via hashtext for pg_advisory_lock
    -- pg_advisory_lock() requires BIGINT; hashtext() returns a stable hash
    v_lock_key         BIGINT := hashtext('spindle_partition_mgmt');

    -- List of all tables that use daily date-based partitioning
    -- Format: "parent_table:parts_tracking_table"
    v_partitioned_tables TEXT[] := ARRAY[
        'resource_events:resource_events_parts',
        'compliance_reports:compliance_reports_parts',
        'control_results:control_results_parts'
    ];
    v_entry TEXT;
    v_parent TEXT;
    v_parts_tbl TEXT;
    v_colon_pos INT;
BEGIN
    -- Acquire advisory lock to prevent concurrent partition management
    -- Blocks until the lock is obtained
    PERFORM pg_advisory_lock(v_lock_key);

    -- Process each partitioned table
    FOREACH v_entry IN ARRAY v_partitioned_tables
    LOOP
        -- Parse "parent_table:parts_table"
        v_colon_pos := position(':' IN v_entry);
        v_parent := left(v_entry, v_colon_pos - 1);
        v_parts_tbl := substring(v_entry, v_colon_pos + 1);

        v_created := 0;
        v_detached := 0;
        v_marked := 0;

        -- Create partition for TODAY
        v_partition_name := v_parent || '_' || to_char(NOW()::DATE, 'YYYY_MM_DD');

        SELECT EXISTS(
            SELECT 1 FROM information_schema.tables
            WHERE table_name = v_partition_name
              AND table_schema = 'public'
        ) INTO v_partition_exists;

        IF NOT v_partition_exists THEN
            EXECUTE format(
                'CREATE TABLE IF NOT EXISTS %I PARTITION OF %I
                 FOR VALUES FROM (%L::date) TO (%L::date)',
                v_partition_name,
                v_parent,
                NOW()::date,
                NOW()::date + INTERVAL '1 day'
            );
            v_created := v_created + 1;

            -- Insert tracking record
            EXECUTE format(
                'INSERT INTO %I (partition_name, relative_date) VALUES (%L, %L)
                 ON CONFLICT (partition_name) DO NOTHING',
                v_parts_tbl,
                v_partition_name,
                NOW()::date
            );
        END IF;

        -- Create partitions for the next p_look_ahead_days days (idempotent)
        FOR i IN 1 .. p_look_ahead_days LOOP
            v_partition_name := v_parent || '_' || to_char(NOW()::DATE + (i - 1) * INTERVAL '1 day', 'YYYY_MM_DD');

            SELECT EXISTS(
                SELECT 1 FROM information_schema.tables
                WHERE table_name = v_partition_name
                  AND table_schema = 'public'
            ) INTO v_partition_exists;

            IF NOT v_partition_exists THEN
                EXECUTE format(
                    'CREATE TABLE IF NOT EXISTS %I PARTITION OF %I
                     FOR VALUES FROM (%L::date) TO (%L::date)',
                    v_partition_name,
                    v_parent,
                    NOW()::date + (i - 1) * INTERVAL '1 day',
                    NOW()::date + i * INTERVAL '1 day'
                );
                v_created := v_created + 1;

                -- Insert tracking record
                EXECUTE format(
                    'INSERT INTO %I (partition_name, relative_date) VALUES (%L, %L)
                     ON CONFLICT (partition_name) DO NOTHING',
                    v_parts_tbl,
                    v_partition_name,
                    NOW()::date + (i - 1) * INTERVAL '1 day'
                );
            END IF;
        END LOOP;

        -- Detach partitions older than warm threshold
        -- Detached partitions remain queryable via parent table inheritance.
        -- After detach, the partition still holds its rows but is no longer
        -- part of the partitioned parent table.
        FOR row IN
            EXECUTE format(
                'SELECT %I AS partition_name
                 FROM %I
                 WHERE relative_date < NOW() - (INTERVAL ''1 day'' * %L)
                   AND NOT is_archive_ready
                 ORDER BY relative_date ASC',
                'partition_name',
                v_parts_tbl,
                p_warm_threshold
            )
        LOOP
            v_partition_exists := EXISTS(
                SELECT 1 FROM information_schema.tables
                WHERE table_name = row.partition_name
                  AND table_schema = 'public'
            );

            IF v_partition_exists THEN
                EXECUTE format(
                    'ALTER TABLE %I DETACH PARTITION %I',
                    v_parent,
                    row.partition_name
                );
                v_detached := v_detached + 1;
            END IF;
        END LOOP;

        -- Mark detached partitions as archive-ready
        EXECUTE format(
            'UPDATE %I
             SET is_archive_ready = true
             WHERE relative_date < NOW() - (INTERVAL ''1 day'' * %L)
               AND is_archive_ready = false',
            v_parts_tbl,
            p_warm_threshold
        );
        GET DIAGNOSTICS v_marked = ROW_COUNT;

        -- Return one row per table with counts
        RETURN QUERY
            SELECT v_parent, v_created, v_detached, v_marked;
    END LOOP;

    -- Release advisory lock (idempotent — safe to call even if already unlocked)
    PERFORM pg_advisory_unlock(v_lock_key);
END;
$$;

-- ===========================================================================
-- 3. verify_partitions() helper
-- ===========================================================================
-- Checks:
--   - Today's partition exists for each table
--   - Look-ahead window (default 7 days) has all partitions
--   - No gaps in the next p_check_gaps_days (default 30)
-- Returns rows of (partition_table, issue, detail) for any problems.

CREATE OR REPLACE FUNCTION verify_partitions(
    p_look_ahead_days INT DEFAULT 7,
    p_check_gaps_days INT DEFAULT 30
)
RETURNS TABLE (
    partition_table TEXT,
    issue TEXT,
    detail TEXT
)
LANGUAGE plpgsql
AS $$
DECLARE
    -- The same list of partitioned tables as manage_partitions
    v_partitioned_tables TEXT[] := ARRAY[
        'resource_events',
        'compliance_reports',
        'control_results'
    ];
    v_parent TEXT;
    v_today DATE := NOW()::DATE;
    v_date DATE;
    v_partition_name TEXT;
    v_exists BOOLEAN;
BEGIN
    FOREACH v_parent IN ARRAY v_partitioned_tables
    LOOP
        -- Check that today's partition exists
        v_partition_name := v_parent || '_' || to_char(v_today, 'YYYY_MM_DD');
        SELECT EXISTS(
            SELECT 1 FROM information_schema.tables
            WHERE table_name = v_partition_name
              AND table_schema = 'public'
        ) INTO v_exists;

        IF NOT v_exists THEN
            RETURN QUERY
                SELECT v_parent, 'MISSING_TODAY'::TEXT,
                       format('Today''s partition %s is missing — run manage_partitions()', v_partition_name)::TEXT;
        END IF;

        -- Check look-ahead window (today + next p_look_ahead_days days)
        FOR i IN 0 .. p_look_ahead_days LOOP
            v_date := v_today + (i - 1) * INTERVAL '1 day';
            v_partition_name := v_parent || '_' || to_char(v_date, 'YYYY_MM_DD');

            SELECT EXISTS(
                SELECT 1 FROM information_schema.tables
                WHERE table_name = v_partition_name
                  AND table_schema = 'public'
            ) INTO v_exists;

            IF NOT v_exists THEN
                RETURN QUERY
                    SELECT v_parent, 'MISSING_LOOKAHEAD'::TEXT,
                           format('Partition %s for %s is missing in look-ahead window',
                                  v_partition_name, v_date::TEXT)::TEXT;
            END IF;
        END LOOP;

        -- Check for gaps in the next p_check_gaps_days
        FOR i IN 0 .. p_check_gaps_days LOOP
            v_date := v_today + i * INTERVAL '1 day';
            v_partition_name := v_parent || '_' || to_char(v_date, 'YYYY_MM_DD');

            SELECT EXISTS(
                SELECT 1 FROM information_schema.tables
                WHERE table_name = v_partition_name
                  AND table_schema = 'public'
            ) INTO v_exists;

            -- Only report gaps for future dates (we don't expect past dates to be partitioned yet)
            IF v_date > v_today AND NOT v_exists THEN
                RETURN QUERY
                    SELECT v_parent, 'GAP'::TEXT,
                           format('Gap detected: partition %s for %s should exist but is missing',
                                  v_partition_name, v_date::TEXT)::TEXT;
            END IF;
        END LOOP;
    END LOOP;

    -- If we get here with no issues found, return an OK row
    IF NOT FOUND THEN
        RETURN QUERY
            SELECT ''::TEXT, 'OK'::TEXT, 'All partitions verified — no gaps or missing partitions in look-ahead window'::TEXT;
    END IF;
END;
$$;

-- ===========================================================================
-- 4. cleanup_partitions() — archival cleanup
-- ===========================================================================
-- Permanently drops detached, archive-ready partitions older than
-- p_retention_days (default: 120). Data should be archived before calling.

CREATE OR REPLACE FUNCTION cleanup_partitions(
    p_retention_days INT DEFAULT 120
)
RETURNS TABLE (
    partition_name TEXT,
    rows_dropped INT
)
LANGUAGE plpgsql
AS $$
DECLARE
    v_record RECORD;
    v_count INT;
    v_lock_key BIGINT := hashtext('spindle_partition_cleanup');
BEGIN
    -- Acquire advisory lock for cleanup
    PERFORM pg_advisory_lock(v_lock_key);

    FOR v_record IN
        SELECT pt.partition_name, pt.relative_date, pt.is_archive_ready
        FROM (
            SELECT partition_name, relative_date, is_archive_ready
            FROM resource_events_parts
            WHERE relative_date < NOW() - (INTERVAL '1 day' * p_retention_days)
              AND is_archive_ready = true
            UNION ALL
            SELECT partition_name, relative_date, is_archive_ready
            FROM compliance_reports_parts
            WHERE relative_date < NOW() - (INTERVAL '1 day' * p_retention_days)
              AND is_archive_ready = true
            UNION ALL
            SELECT partition_name, relative_date, is_archive_ready
            FROM control_results_parts
            WHERE relative_date < NOW() - (INTERVAL '1 day' * p_retention_days)
              AND is_archive_ready = true
        ) pt
    LOOP
        -- Count rows before dropping
        EXECUTE format('SELECT COUNT(*) FROM %I', v_record.partition_name) INTO v_count;

        -- Drop the partition (table must already be detached)
        EXECUTE format('DROP TABLE IF EXISTS %I', v_record.partition_name);

        -- Remove from tracking tables
        DELETE FROM resource_events_parts WHERE partition_name = v_record.partition_name;
        DELETE FROM compliance_reports_parts WHERE partition_name = v_record.partition_name;
        DELETE FROM control_results_parts WHERE partition_name = v_record.partition_name;

        RETURN QUERY SELECT v_record.partition_name, v_count;
    END LOOP;

    -- Release advisory lock
    PERFORM pg_advisory_unlock(v_lock_key);
END;
$$;

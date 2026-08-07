-- Migration 003: Partition management
-- Purpose: Idempotent partition lifecycle for partitioned event tables
--         Create next 7 days of partitions, detach old ones (90d warm threshold),
--         mark archive-ready partitions, keep them queryable via inheritance
-- Called by: worker cron job
-- Rollback: N/A (forward-only)
-- Replay: Re-run from archive if schema version is out of sync
--
-- Warm threshold: partitions older than this remain queryable but are detached
--   from the parent table (so they can be safely archived/deleted).
-- Archive-ready: detached partitions that have been safely detached and can be
--   removed from the parent table entirely once archival is confirmed.
--
-- The resource_events_parts tracking table must exist BEFORE this migration
-- runs. It should be created in a prior migration (e.g. migration 002).

CREATE OR REPLACE FUNCTION manage_partitions(
    p_look_ahead_days   INT         DEFAULT 7,
    p_warm_threshold    INT         DEFAULT 90
)
RETURNS TABLE (
    created     INT,
    detached    INT,
    marked      INT
)
LANGUAGE plpgsql
VOLATILE  -- side effects on every call (CREATE, ALTER, UPDATE)
SECURITY DEFINER
AS $$
DECLARE
    v_created      INT := 0;
    v_detached     INT := 0;
    v_marked       INT := 0;

    v_partition_name   TEXT;
    v_partition_exists BOOLEAN;

    v_lock_key     BIGINT := hashtext('spindle_partition_mgmt');
BEGIN
    -- Advisory lock with a FIXED bigint key so concurrent runs block each other.
    -- pg_advisory_lock() requires BIGINT; hashtext() returns a stable hash.
    PERFORM pg_advisory_lock(v_lock_key);

    -- Create today's partition
    v_partition_name := 'resource_events_' || to_char(NOW()::DATE, 'YYYY_MM_DD');

    SELECT EXISTS(
        SELECT 1 FROM information_schema.tables
        WHERE table_name = v_partition_name
          AND table_schema = 'public'
    ) INTO v_partition_exists;

    IF NOT v_partition_exists THEN
        EXECUTE format(
            'CREATE TABLE IF NOT EXISTS %I PARTITION OF resource_events
               FOR VALUES FROM (%L::date) TO (%L::date)',
            v_partition_name,
            NOW()::date,
            NOW()::date + INTERVAL '1 day'
        );
        v_created := v_created + 1;
    END IF;

    -- Create partitions for the next p_look_ahead_days days (idempotent)
    FOR i IN 1 .. p_look_ahead_days LOOP
        v_partition_name := 'resource_events_' || to_char(NOW()::DATE + (i - 1) * INTERVAL '1 day', 'YYYY_MM_DD');

        SELECT EXISTS(
            SELECT 1 FROM information_schema.tables
            WHERE table_name = v_partition_name
              AND table_schema = 'public'
        ) INTO v_partition_exists;

        IF NOT v_partition_exists THEN
            EXECUTE format(
                'CREATE TABLE IF NOT EXISTS %I PARTITION OF resource_events
                   FOR VALUES FROM (%L::date) TO (%L::date)',
                v_partition_name,
                NOW()::date + (i - 1) * INTERVAL '1 day',
                NOW()::date + i * INTERVAL '1 day'
            );
            v_created := v_created + 1;
        END IF;
    END LOOP;

    -- Detach partitions older than warm threshold
    -- Detached partitions remain queryable via parent table inheritance.
    -- After detach, the partition still holds its rows but is no longer
    -- part of the partitioned parent table.
    FOR row IN
        SELECT 'resource_events_' || to_char(relative_date::date, 'YYYY_MM_DD') AS partition_name,
               relative_date
        FROM   resource_events_parts
        WHERE  relative_date < NOW() - (INTERVAL '1 day' * p_warm_threshold)
          AND   NOT is_archive_ready
        ORDER BY relative_date ASC
    LOOP
        v_partition_exists := EXISTS(
            SELECT 1 FROM information_schema.tables
            WHERE table_name = row.partition_name
              AND table_schema = 'public'
        );

        IF v_partition_exists THEN
            EXECUTE format(
                'ALTER TABLE resource_events DETACH PARTITION %I',
                row.partition_name
            );
            v_detached := v_detached + 1;
        END IF;
    END LOOP;

    -- Mark detached partitions as archive-ready
    UPDATE resource_events_parts
    SET is_archive_ready = true
    WHERE   relative_date < NOW() - (INTERVAL '1 day' * p_warm_threshold)
      AND   is_archive_ready = false;
    GET DIAGNOSTICS v_marked = ROW_COUNT;

    -- Release advisory lock (no-op on already-unlocked)
    PERFORM pg_advisory_unlock(v_lock_key);

    RETURN QUERY
        SELECT v_created, v_detached, v_marked;
END;
$$;

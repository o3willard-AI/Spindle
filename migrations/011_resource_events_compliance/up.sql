-- Migration 011: resource_events + compliance tables with daily partitioning
-- Requirements: STO-01, STO-02
-- Description: Creates resource_events, compliance_reports, and control_results tables
-- with daily partitioning and partition management function.

-- Create partition management function
CREATE OR REPLACE FUNCTION manage_partitions()
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    v_partition_date DATE;
    v_days_ahead INTEGER := 7;
BEGIN
    -- Create partitions for the next 7 days if they don't exist
    FOR i IN 1..v_days_ahead LOOP
        v_partition_date := CURRENT_DATE + i;
        
        -- Create resource_events partition
        IF NOT EXISTS (
            SELECT 1 FROM information_schema.partitions 
            WHERE table_schema = 'public' 
            AND table_name = 'resource_events' 
            AND partition_name = 'resource_events_' || TO_CHAR(v_partition_date, 'YYYY_MM_DD')
        ) THEN
            EXECUTE format(
                'ALTER TABLE resource_events ATTACH PARTITION resource_events_%s FOR VALUES (%L)',
                TO_CHAR(v_partition_date, 'YYYY_MM_DD'),
                'DAY'
            );
        END IF;
        
        -- Create compliance_reports partition
        IF NOT EXISTS (
            SELECT 1 FROM information_schema.partitions 
            WHERE table_schema = 'public' 
            AND table_name = 'compliance_reports' 
            AND partition_name = 'compliance_reports_' || TO_CHAR(v_partition_date, 'YYYY_MM_DD')
        ) THEN
            EXECUTE format(
                'ALTER TABLE compliance_reports ATTACH PARTITION compliance_reports_%s FOR VALUES (%L)',
                TO_CHAR(v_partition_date, 'YYYY_MM_DD'),
                'DAY'
            );
        END IF;
        
        -- Create control_results partition
        IF NOT EXISTS (
            SELECT 1 FROM information_schema.partitions 
            WHERE table_schema = 'public' 
            AND table_name = 'control_results' 
            AND partition_name = 'control_results_' || TO_CHAR(v_partition_date, 'YYYY_MM_DD')
        ) THEN
            EXECUTE format(
                'ALTER TABLE control_results ATTACH PARTITION control_results_%s FOR VALUES (%L)',
                TO_CHAR(v_partition_date, 'YYYY_MM_DD'),
                'DAY'
            );
        END IF;
    END LOOP;
END;
$$;

-- Create resource_events table
CREATE TABLE resource_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    resource_type TEXT NOT NULL,
    resource_name TEXT NOT NULL,
    action TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('updated', 'failed', 'skipped')),
    duration_ms INT NOT NULL,
    cookbook_name TEXT,
    cookbook_version TEXT,
    guard_outcome JSONB,
    delta JSONB,
    schema_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
) PARTITION BY RANGE (created_at);

-- Create BRIN index on resource_events
CREATE INDEX idx_resource_events_created_at ON resource_events USING brin (created_at);

-- Create compliance_reports table
CREATE TABLE compliance_reports (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    profile_name TEXT NOT NULL,
    profile_version TEXT,
    control_id TEXT NOT NULL,
    control_title TEXT,
    status TEXT NOT NULL CHECK (status IN ('passed', 'failed', 'error', 'not_applicable')),
    result JSONB NOT NULL,
    resource_type TEXT,
    resource_name TEXT,
    cookbook_name TEXT,
    cookbook_version TEXT,
    guard_outcome JSONB,
    schema_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
) PARTITION BY RANGE (created_at);

-- Create BRIN index on compliance_reports
CREATE INDEX idx_compliance_reports_created_at ON compliance_reports USING brin (created_at);

-- Create control_results table
CREATE TABLE control_results (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    control_id TEXT NOT NULL,
    control_title TEXT,
    status TEXT NOT NULL CHECK (status IN ('passed', 'failed', 'error', 'not_applicable')),
    result JSONB NOT NULL,
    resource_type TEXT,
    resource_name TEXT,
    cookbook_name TEXT,
    cookbook_version TEXT,
    guard_outcome JSONB,
    schema_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
) PARTITION BY RANGE (created_at);

-- Create BRIN index on control_results
CREATE INDEX idx_control_results_created_at ON control_results USING brin (created_at);

-- Create partitions for the next 7 days
SELECT manage_partitions();

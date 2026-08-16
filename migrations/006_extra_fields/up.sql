-- Migration 018: extra_fields JSONB columns + GIN indexes
-- Purpose: Add extra_fields column to all tables that store parsed payload data,
--   plus GIN indexes for ad-hoc JSON queries on unknown fields.
-- Supports M1-24: Unknown field preservation via #[serde(flatten)].
--
-- Schema evolution: When payloads arrive with fields not yet in the schema,
-- they land in extra_fields. Queries can inspect them with @>, ->, ->>, etc.
-- To promote an extra field to a typed column, add it to the Rust struct and
-- create a new migration adding the corresponding DB column.

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ resource_events (partitioned by day)                                │
-- └─────────────────────────────────────────────────────────────────────┘

ALTER TABLE resource_events ADD COLUMN IF NOT EXISTS extra_fields JSONB DEFAULT '{}';

-- GIN index for ad-hoc JSON path queries on unknown resource event fields
CREATE INDEX IF NOT EXISTS idx_resource_events_extra_fields
    ON resource_events USING GIN (extra_fields);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ compliance_reports                                                   │
-- └─────────────────────────────────────────────────────────────────────┘

ALTER TABLE compliance_reports ADD COLUMN IF NOT EXISTS extra_fields JSONB DEFAULT '{}';

-- GIN index for ad-hoc JSON path queries on unknown compliance report fields
CREATE INDEX IF NOT EXISTS idx_compliance_reports_extra_fields
    ON compliance_reports USING GIN (extra_fields);

-- ┌─────────────────────────────────────────────────────────────────────┐
-- │ control_results (partitioned by day)                                │
-- └─────────────────────────────────────────────────────────────────────┘

ALTER TABLE control_results ADD COLUMN IF NOT EXISTS extra_fields JSONB DEFAULT '{}';

-- GIN index for ad-hoc JSON path queries on unknown control result fields
CREATE INDEX IF NOT EXISTS idx_control_results_extra_fields
    ON control_results USING GIN (extra_fields);

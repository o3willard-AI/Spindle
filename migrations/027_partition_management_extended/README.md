# Migration 013: Partition Management — Extended Support

**Purpose:** Extend `manage_partitions()` to handle ALL partitioned tables (resource_events, compliance_reports, control_results) with daily date-based partitioning. Adds tracking tables, `verify_partitions()` helper, and `cleanup_partitions()` for archival.

**Requirements:** STO-02 (M1-07)

**Called by:** Worker cron job

## What was implemented

### 1. Tracking tables
- `compliance_reports_parts` — tracks partition state for compliance_reports (mirrors resource_events_parts structure)
- `control_results_parts` — tracks partition state for control_results
- `resource_events_parts` — already exists from migration 001

### 2. `manage_partitions(p_look_ahead_days, p_warm_threshold)`
- Creates daily partitions for next 7 days (configurable) for ALL three tables
- Partition naming: `{table_name}_YYYY_MM_DD`
- Detaches partitions older than 90 days (warm threshold)
- Marked as archive-ready after detach
- Uses `pg_advisory_lock` with fixed key for concurrency safety
- **Idempotent** — safe to run multiple times
- Returns per-table counts: (partition_table, created, detached, marked_archive)

### 3. `verify_partitions(p_look_ahead_days, p_check_gaps_days)`
- Checks today's partition exists for each table
- Validates look-ahead window (default 7 days)
- Scans for gaps in the next 30 days
- Returns (partition_table, issue, detail) for any problems

### 4. `cleanup_partitions(p_retention_days)`
- Permanently drops detached, archive-ready partitions older than 120 days
- Counts rows before dropping for audit
- Cleans up tracking table entries

## Design decisions
- **Advisory lock** with `hashtext('spindle_partition_mgmt')` for stable bigint key
- **Table list** as `parent_table:parts_table` array, parsed with `position()` and `substring()`
- **All CREATE TABLE IF NOT EXISTS** — safe for re-runs
- **ON CONFLICT DO NOTHING** for tracking table inserts
- **Union ALL** in cleanup for combining all tracking tables

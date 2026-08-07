# Migration 013: Partition Management — Extended Support

**Purpose:** Extend `manage_partitions()` to handle all three partitioned tables (resource_events, compliance_reports, control_results) with daily date-based partitioning. Adds tracking tables, `verify_partitions()` helper, and `cleanup_partitions()` for archival.

**Requirements:** STO-02 (M1-07)

## What was implemented

### 1. Tracking tables for additional partitioned tables
- `compliance_reports_parts` — mirrors `resource_events_parts` structure
- `control_results_parts` — mirrors `resource_events_parts` structure
- Each tracks: partition_name, relative_date, row counts, archive_ready flag

### 2. Extended `manage_partitions()` function
- Replaces Sergey's original version (only handled resource_events)
- Now handles all three tables: `resource_events`, `compliance_reports`, `control_results`
- Partition naming: `{table_name}_YYYY_MM_DD`
- Creates partitions for today + next `p_look_ahead_days` (default: 7)
- Detaches partitions older than `p_warm_threshold` (default: 90 days)
- Uses advisory lock (`pg_advisory_lock`) to prevent concurrent execution
- Idempotent — safe to run multiple times (no duplicate partitions)
- Returns per-table counts as TABLE result: (partition_table, created, detached, marked_archive)

### 3. `verify_partitions()` helper
- Checks today's partition exists for each table
- Validates look-ahead window (default: 7 days ahead)
- Scans for gaps in the next 30 days
- Returns (partition_table, issue, detail) for any problems

### 4. `cleanup_partitions()` function
- Permanently drops detached, archive-ready partitions older than `p_retention_days` (default: 120)
- Counts rows before dropping for audit purposes
- Removes entries from tracking tables

## Key design decisions

- **Advisory lock** uses `hashtext('spindle_partition_mgmt')` for stable bigint key
- **Partition tables list** is an array of strings parsed as `table:parts_table:date_column`
- **Conditional DDL** not needed — all CREATE TABLE IF NOT EXISTS and CREATE FUNCTION OR REPLACE
- **Tracking table inserts** use ON CONFLICT DO NOTHING for idempotency
- **Date ranges** computed via `NOW()::date + INTERVAL '1 day' * i`
- **Partition of** syntax requires PostgreSQL 10+ (declarative partitioning)

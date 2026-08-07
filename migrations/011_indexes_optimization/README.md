# Migration 011: Indexes + Query Optimization

**Purpose:** Add missing composite, partial, and GIN indexes for optimal query performance across all Spindle tables.

**Requirements:** STO-03, STO-04 (M1-08)

**Rollback:** Drop individual indexes as needed. No table modifications.

## Index Categories

| Table | Index Type | Count |
|-------|-----------|-------|
| nodes | Composite | 3 |
| nodes | GIN | 1 |
| nodes | Single-column | 3 |
| runs | Composite | 4 |
| runs | Partial | 2 |
| runs | GIN | 2 |
| runs | Single-column | 1 |
| resource_events | Composite | 3 |
| resource_events | Partial | 1 |
| resource_events | BRIN | 1 |
| resource_events | Single-column | 1 |
| compliance_reports | Single-column | 1 |
| compliance_reports | Composite | 2 |
| compliance_reports | BRIN | 1 |
| control_results | Single-column | 2 |
| control_results | Composite | 1 |
| control_results | BRIN | 1 |
| profiles | Single-column | 2 |
| waivers | Single-column | 2 |
| waivers | Composite | 1 |
| cookbook_usage | Single-column | 2 |
| cookbook_usage | Composite | 1 |
| duration_rollups | Single-column | 3 |
| audit_log | Single-column | 2 |
| audit_log | Composite | 2 |
| audit_log | BRIN | 0 |

**Total indexes added:** 35

## Design Decisions

- **BRIN indexes** on `created_at`/`start_time` for time-ordered tables (resource_events, control_results, compliance_reports) — efficient for large append-only tables
- **Expression indexes** on JSONB extraction (`attributes->>'field'`) in M1-04 for nodes; GIN indexes added here for ad-hoc JSONB queries
- **Partial indexes** only on `status = 'failed'` (high-value for troubleshooting) and `status = 'compliance'` (large table scan reduction)
- **Composite indexes** ordered to match common WHERE + ORDER BY + LIMIT query patterns
- **ANALYZE** at end of migration — no PostgreSQL available for live testing; planner statistics will be accurate after first ANALYZE

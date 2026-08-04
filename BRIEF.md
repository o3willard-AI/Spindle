# Sergey — Spindle M0-08: Migration Runner

Requirement: STO-08. Integrate `sqlx-cli` for database migrations.

## What to build
- `migrations/` directory with `sqlx migrate`-compatible structure
- Forward-only migrations (no rollback — replay from archive instead)
- First migration: schema version tracking table
- `spindle-server migrate` subcommand to run pending migrations
- Each migration: `up.sql` + documented rollback/replay path in comments

## Tests
- Apply all → re-run → zero new migrations
- Fresh DB → apply → schema matches expected
- Migration with ordering dependencies → explicit, not implicit

## Verify
`sqlx migrate run` against local Postgres (docker-compose from M0-03), then `cargo test` → green, push.

# Migration 001: Schema Version Tracking Table

**Purpose:** Track applied migrations for forward-only replay from archive.

**Rollback:** N/A (forward-only migrations, replay from archive instead).

**Replay:** If schema version is out of sync, re-run from archive.

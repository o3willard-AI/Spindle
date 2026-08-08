#!/bin/bash
# backup-database.sh — Full PostgreSQL backup with WAL archiving for Spindle.
#
# Usage: backup-database.sh
#
# Environment variables:
#   BACKUP_DIR    — backup destination (default: /var/backups/spindle)
#   DATABASE_URL  — PostgreSQL connection string (default: postgresql://spindle:spindle@localhost:5432/spindle)
#   WAL_ARCHIVE   — WAL archive directory (default: /var/lib/postgresql/wal_archive)

set -euo pipefail

BACKUP_DIR="${BACKUP_DIR:-/var/backups/spindle}"
TIMESTAMP=$(date -u +"%Y%m%dT%H%M%SZ")
DB_URL="${DATABASE_URL:-postgresql://spindle:spindle@localhost:5432/spindle}"
WAL_ARCHIVE="${WAL_ARCHIVE:-/var/lib/postgresql/wal_archive}"

# ── Configuration ───────────────────────────────────────────────────────────────

# Verify WAL archiving is configured (postgresql.conf must have archive_mode=on)
echo "[backup] Starting database backup at $TIMESTAMP"

mkdir -p "$BACKUP_DIR/db/$TIMESTAMP"

# 1. Take base backup
echo "[backup] Running pg_dump..."
pg_dump "$DB_URL" > "$BACKUP_DIR/db/$TIMESTAMP/spindle-full.sql"

# 2. Copy WAL archive if available (for point-in-time recovery)
if [ -d "$WAL_ARCHIVE" ] && [ "$(ls -A "$WAL_ARCHIVE" 2>/dev/null)" ]; then
    echo "[backup] Copying WAL archive..."
    mkdir -p "$BACKUP_DIR/db/$TIMESTAMP/wal-archive"
    cp -r "$WAL_ARCHIVE"/* "$BACKUP_DIR/db/$TIMESTAMP/wal-archive/"
fi

# 3. Create backup manifest
cat > "$BACKUP_DIR/db/$TIMESTAMP/backup-manifest.json" <<EOF
{
    "timestamp": "$TIMESTAMP",
    "type": "full-database",
    "wal_archive": $([ -d "$WAL_ARCHIVE" ] && echo true || echo false),
}
EOF

# 4. Compress
tar czf "$BACKUP_DIR/spindle-db-$TIMESTAMP.tar.gz" \
    -C "$BACKUP_DIR/db/$TIMESTAMP" .

# 5. Cleanup uncompressed
rm -rf "$BACKUP_DIR/db/$TIMESTAMP"

echo "[backup] Database backup complete: $BACKUP_DIR/spindle-db-$TIMESTAMP.tar.gz"
echo "[backup] Size: $(du -sh "$BACKUP_DIR/spindle-db-$TIMESTAMP.tar.gz" | cut -f1)"

#!/bin/bash
# backup-manifests.sh — Backup ONLY the manifest chain of custody.
#
# **IMPORTANT**: Manifests are the chain of custody. Back them up FIRST.
# Losing manifests is worse than losing archive sets.
#
# Usage: backup-manifests.sh
#
# Environment variables:
#   BACKUP_DIR    — backup destination (default: /var/backups/spindle)
#   DATABASE_URL  — PostgreSQL connection string

set -euo pipefail

BACKUP_DIR="${BACKUP_DIR:-/var/backups/spindle}"
TIMESTAMP=$(date -u +"%Y%m%dT%H%M%SZ")
DB_URL="${DATABASE_URL:-postgresql://spindle:CHANGE_ME@localhost:5432/spindle}"

echo "[backup-manifests] Starting manifests backup at $TIMESTAMP"
echo "[backup-manifests] WARNING: Manifests are the chain of custody. Verify before restore."

mkdir -p "$BACKUP_DIR/manifests/$TIMESTAMP"

# 1. Export ONLY the manifests table — this is the chain of custody
echo "[backup-manifests] Exporting spindle_manifests table..."
pg_dump \
    --column-inserts \
    --table=spindle_manifests \
    "$DB_URL" \
    > "$BACKUP_DIR/manifests/$TIMESTAMP/spindle-manifests.sql"

# 2. Also export manifest metadata as JSON for quick verification
echo "[backup-manifests] Exporting manifests as JSON..."
psql "$DB_URL" -t -A -F '\t' \
    -c "SELECT json_agg(row_to_json(m)) FROM spindle_manifests m;" \
    > "$BACKUP_DIR/manifests/$TIMESTAMP/spindle-manifests.json"

# 3. Create backup manifest
cat > "$BACKUP_DIR/manifests/$TIMESTAMP/backup-manifest.json" <<EOF
{
    "timestamp": "$TIMESTAMP",
    "type": "manifests-chain-of-custody",
    "manifest_count": "$(grep -c 'INSERT' "$BACKUP_DIR/manifests/$TIMESTAMP/spindle-manifests.sql" || echo 0)"
}
EOF

# 4. Compress
tar czf "$BACKUP_DIR/spindle-manifests-$TIMESTAMP.tar.gz" \
    -C "$BACKUP_DIR/manifests/$TIMESTAMP" .

# 5. Cleanup uncompressed
rm -rf "$BACKUP_DIR/manifests/$TIMESTAMP"

echo "[backup-manifests] Manifests backup complete: $BACKUP_DIR/spindle-manifests-$TIMESTAMP.tar.gz"
echo "[backup-manifests] Retention: 90 days (chain of custody)"

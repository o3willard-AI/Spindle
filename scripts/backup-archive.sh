#!/bin/bash
# backup-archive.sh — Backup raw archive (filesystem or S3/MinIO).
#
# Usage: backup-archive.sh
#
# Environment variables:
#   BACKUP_DIR    — backup destination (default: /var/backups/spindle)
#   ARCHIVE_DIR   — local archive directory (default: /var/lib/spindle/raw-archive)
#   S3_BUCKET     — S3 bucket name (if using S3/MinIO)
#   S3_ENDPOINT   — S3 endpoint URL (if using S3/MinIO)
#   REMOTE_ARCHIVE — rclone remote path (if using rclone)

set -euo pipefail

BACKUP_DIR="${BACKUP_DIR:-/var/backups/spindle}"
TIMESTAMP=$(date -u +"%Y%m%dT%H%M%SZ")
ARCHIVE_DIR="${ARCHIVE_DIR:-/var/lib/spindle/raw-archive}"

echo "[backup-archive] Starting archive backup at $TIMESTAMP"

mkdir -p "$BACKUP_DIR/archive/$TIMESTAMP"

# Option A: If archive is on local filesystem
if [ -d "$ARCHIVE_DIR" ] && [ "$(ls -A "$ARCHIVE_DIR" 2>/dev/null)" ]; then
    echo "[backup-archive] Backing up from local filesystem: $ARCHIVE_DIR"
    rsync -av --delete "$ARCHIVE_DIR/" "$BACKUP_DIR/archive/$TIMESTAMP/raw/"
fi

# Option B: If archive is on S3/MinIO
if [ -n "${S3_BUCKET:-}" ]; then
    echo "[backup-archive] Backing up from S3: s3://$S3_BUCKET/spindle-archive/"
    mkdir -p "$BACKUP_DIR/archive/$TIMESTAMP/s3"
    if [ -n "${S3_ENDPOINT:-}" ]; then
        aws s3 sync "s3://$S3_BUCKET/spindle-archive/" \
            "$BACKUP_DIR/archive/$TIMESTAMP/s3/" \
            --endpoint-url "$S3_ENDPOINT"
    else
        aws s3 sync "s3://$S3_BUCKET/spindle-archive/" \
            "$BACKUP_DIR/archive/$TIMESTAMP/s3/"
    fi
fi

# Option C: If archive is on a remote host via rclone
if [ -n "${REMOTE_ARCHIVE:-}" ]; then
    echo "[backup-archive] Backing up from rclone remote: $REMOTE_ARCHIVE"
    mkdir -p "$BACKUP_DIR/archive/$TIMESTAMP/rclone"
    rclone sync "$REMOTE_ARCHIVE" "$BACKUP_DIR/archive/$TIMESTAMP/rclone/"
fi

# 2. Create backup manifest
cat > "$BACKUP_DIR/archive/$TIMESTAMP/backup-manifest.json" <<EOF
{
    "timestamp": "$TIMESTAMP",
    "type": "raw-archive",
    "source": "filesystem",
    "archive_path": "$ARCHIVE_DIR"
}
EOF

# 3. Compress
tar czf "$BACKUP_DIR/spindle-archive-$TIMESTAMP.tar.gz" \
    -C "$BACKUP_DIR/archive/$TIMESTAMP" .

# 4. Cleanup uncompressed
rm -rf "$BACKUP_DIR/archive/$TIMESTAMP"

echo "[backup-archive] Archive backup complete: $BACKUP_DIR/spindle-archive-$TIMESTAMP.tar.gz"
echo "[backup-archive] Size: $(du -sh "$BACKUP_DIR/spindle-archive-$TIMESTAMP.tar.gz" | cut -f1)"

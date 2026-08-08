# Spindle Backup & Restore Procedure

## Overview

Spindle has three critical data domains that must be backed up:

1. **Database** (PostgreSQL) — node state, run metadata, compliance data, identity mappings, tokens
2. **Raw archive** (object storage / filesystem) — raw ingest messages, compliance reports
3. **Archive manifests** (database table `spindle_manifests`) — SHA-256 hashes + signed manifest chain of custody

> **⚠️ MANIFESTS ARE THE CHAIN OF CUSTODY. BACK THEM UP FIRST. LOSING MANIFESTS IS WORSE THAN LOSING ARCHIVE SETS.**

Without manifests, you can restore raw data but cannot verify its integrity or
authenticate the chain of custody. With manifests but no raw archive, you can
re-derive compliance data from ingest replays. Both should be backed up, but
**manifests take priority**.

## Backup Strategy

### Daily backup schedule

| Component | Tool | Frequency | Retention |
|---|---|---|---|
| Database | `pg_dump` + WAL archiving | Daily full, hourly WAL | 30 days |
| Manifests | `pg_dump --table=spindle_manifests` | Daily (before archive backup) | 90 days |
| Raw archive | `rclone sync` or `aws s3 sync` | Daily | 30 days |
| Signing keys | Manual copy (offline) | After rotation | Indefinite |

### 1. Database backup

```bash
#!/bin/bash
# backup-database.sh — Full PostgreSQL backup with WAL archiving

set -euo pipefail

BACKUP_DIR="${BACKUP_DIR:-/var/backups/spindle}"
TIMESTAMP=$(date -u +"%Y%m%dT%H%M%SZ")
DB_URL="${DATABASE_URL:-postgresql://spindle:spindle@localhost:5432/spindle}"

mkdir -p "$BACKUP_DIR/$TIMESTAMP"

# 1. Ensure WAL archiving is enabled (postgresql.conf)
#    archive_mode = on
#    archive_command = 'cp %p /var/lib/postgresql/wal_archive/%f'

# 2. Take base backup
pg_dump "$DB_URL" > "$BACKUP_DIR/$TIMESTAMP/spindle-full.sql"

# 3. Export WAL segment list for point-in-time recovery
if [ -d /var/lib/postgresql/wal_archive ]; then
    cp -r /var/lib/postgresql/wal_archive "$BACKUP_DIR/$TIMESTAMP/wal-archive/"
fi

# 4. Create backup manifest
cat > "$BACKUP_DIR/$TIMESTAMP/backup-manifest.json" <<EOF
{
    "timestamp": "$TIMESTAMP",
    "type": "full-database",
    "db_dump": "spindle-full.sql",
    "has_wal": $([ -d /var/lib/postgresql/wal_archive ] && echo true || echo false),
    "spindle_version": "$(spindle-server --version 2>/dev/null || echo unknown)"
}
EOF

# 5. Compress
tar czf "$BACKUP_DIR/spindle-db-$TIMESTAMP.tar.gz" -C "$BACKUP_DIR/$TIMESTAMP" .

echo "Database backup complete: $BACKUP_DIR/spindle-db-$TIMESTAMP.tar.gz"
```

### 2. Manifests backup (priority)

```bash
#!/bin/bash
# backup-manifests.sh — Backup ONLY the manifest chain of custody

set -euo pipefail

BACKUP_DIR="${BACKUP_DIR:-/var/backups/spindle}"
TIMESTAMP=$(date -u +"%Y%m%dT%H%M%SZ")
DB_URL="${DATABASE_URL:-postgresql://spindle:spindle@localhost:5432/spindle}"

mkdir -p "$BACKUP_DIR/manifests/$TIMESTAMP"

# Export ONLY the manifests table — this is the chain of custody
pg_dump \
    --column-inserts \
    --table=spindle_manifests \
    "$DB_URL" \
    > "$BACKUP_DIR/manifests/$TIMESTAMP/spindle-manifests.sql"

# Also export manifest metadata as JSON
psql "$DB_URL" -t -A -F '\t' \
    -c "SELECT json_agg(row_to_json(m)) FROM spindle_manifests m;" \
    > "$BACKUP_DIR/manifests/$TIMESTAMP/spindle-manifests.json"

# Sign the backup for tamper evidence
# Key generation handled by `spindle keys generate` — see Keys section below

# Note: `spindle keys rotate` only rotates keys; it does NOT sign arbitrary payloads.
# To sign manifest backups, use:
echo "$BACKUP_DIR/manifests/$TIMESTAMP/spindle-manifests.json" |     xargs -I {} spindle keys verify --path /opt/spindle/backup-key.aes --file {}

tar czf "$BACKUP_DIR/manifests-$TIMESTAMP.tar.gz" -C "$BACKUP_DIR/manifests/$TIMESTAMP" .

echo "Manifests backup complete: $BACKUP_DIR/manifests-$TIMESTAMP.tar.gz"
```

### 3. Raw archive backup

```bash
#!/bin/bash
# backup-archive.sh — Backup raw archive (filesystem or S3/MinIO)

set -euo pipefail

BACKUP_DIR="${BACKUP_DIR:-/var/backups/spindle}"
TIMESTAMP=$(date -u +"%Y%m%dT%H%M%SZ")
ARCHIVE_DIR="${ARCHIVE_DIR:-/var/lib/spindle/raw-archive}"

mkdir -p "$BACKUP_DIR/archive/$TIMESTAMP"

# Option A: If archive is on filesystem
if [ -d "$ARCHIVE_DIR" ]; then
    rsync -av --delete "$ARCHIVE_DIR/" "$BACKUP_DIR/archive/$TIMESTAMP/raw/"
fi

# Option B: If archive is on S3/MinIO
if [ -n "${S3_BUCKET:-}" ]; then
    aws s3 sync "s3://$S3_BUCKET/spindle-archive/" "$BACKUP_DIR/archive/$TIMESTAMP/s3/"
fi

# Option C: If archive is on a remote host
if [ -n "${REMOTE_ARCHIVE:-}" ]; then
    rclone sync "remote:$REMOTE_ARCHIVE" "$BACKUP_DIR/archive/$TIMESTAMP/rclone/"
fi

tar czf "$BACKUP_DIR/spindle-archive-$TIMESTAMP.tar.gz" -C "$BACKUP_DIR/archive/$TIMESTAMP" .

echo "Archive backup complete: $BACKUP_DIR/spindle-archive-$TIMESTAMP.tar.gz"
```

### 4. Signing key backup (offline)

```bash
# Copy signing key to offline storage
sudo cp /opt/spindle/signing-key.aes /mnt/offline-storage/spindle-key-$(date +%Y%m%d).aes
# Verify the copy matches
sha256sum /opt/spindle/signing-key.aes /mnt/offline-storage/spindle-key-$(date +%Y%m%d).aes
```

## Restore Procedure

### Before restore

1. **Stop all Spindle services**
   ```bash
   sudo systemctl stop spindle-server
   sudo systemctl stop spindle-worker
   ```

2. **Document current state**
   ```bash
   # Record the last processed run IDs for verification
   spindle runs list --output json > /tmp/runs-before-restore.json
   ```

### Restore Steps (in order)

#### Step 1: Restore manifests (chain of custody)

```bash
# Restore the manifests table FIRST — before anything else
tar xzf /var/backups/spindle/manifests-20240101T120000Z.tar.gz
psql "$DATABASE_URL" -f spindle-manifests.sql
```

#### Step 2: Restore database

```bash
# Option A: Full restore from base backup (with PITR)
pg_restore --Clean --if-exists /var/backups/spindle/db-backup.tar

# Option B: Point-in-time recovery from base + WAL
pg_basebackup -D /var/lib/postgresql/data -Fp -Xs -P -R
# Restore WAL segments
cp -r /var/backups/spindle/wal-archive/* /var/lib/postgresql/wal_archive/
```

#### Step 3: Restore raw archive

```bash
# Restore from filesystem backup
rsync -av /var/backups/spindle/archive-20240101T120000Z/raw/ /var/lib/spindle/raw-archive/

# Or from S3 backup
aws s3 sync /var/backups/spindle/archive-20240101T120000Z/s3/ s3://your-bucket/spindle-archive/
```

#### Step 4: Verify integrity

```bash
# Verify manifests match archive contents
spindle archive verify --path /var/lib/spindle/raw-archive/2024-W01

# Cross-verify with Python script (independent verification)
python3 tools/verify_spindle_archive.py \
    --keys-url http://localhost:3000/keys.json \
    --archive /var/lib/spindle/raw-archive/2024-W01
```

#### Step 5: Start services

```bash
sudo systemctl start spindle-server
sudo systemctl start spindle-worker

# Verify health
spindle health
# Expected: exit code 0
```

#### Step 6: Replay and verify

```bash
# Run a corpus replay to verify restored data is complete
spindle compliance export --report-type control_status_by_node > /tmp/post-restore-export.json

# Compare with pre-restore export
# Pre-restore export captured earlier — compare with current state
cat /tmp/pre-backup-export.json

diff /tmp/pre-restore-export.json /tmp/post-restore-export.json
# Should be identical
```

## CI Test Procedure (aspirational)

The following automation target is documented for future implementation. Scripts
in `scripts/` provide manual execution; CI integration is planned once core
backfill pipelines stabilize.

**Available manual scripts:**

| Script | Purpose | Status |
|---|---|---|
| `scripts/backup-database.sh` | Full PostgreSQL backup + WAL export | ✅ Exists |
| `scripts/backup-manifests.sh` | Manifests table export + JSON | ✅ Exists |
| `scripts/backup-archive.sh` | Raw archive sync (FS/S3/remote) | ✅ Exists |
| `scripts/restore-database.sh` | Database restore + PITR replay | ⚠️ Not yet implemented |
| `scripts/restore-archive.sh` | Archive restoration | ⚠️ Not yet implemented |
| `scripts/test-corpus.py` | Synthetic corpus generator | ❌ Not yet implemented |

Planned CI workflow: `.github/workflows/backup-restore-test.yml` — not yet active.

Manual test procedure:

1. **Pre-export** (capture current state before backup)
   ```bash
   spindle compliance export --report-type control_status_by_node > /tmp/pre-backup-export.json
   ```

2. **Run backups**
   ```bash
   bash scripts/backup-database.sh
   bash scripts/backup-manifests.sh
   bash scripts/backup-archive.sh
   ```

3. **Destroy state** (manual disaster recovery drill)
   ```bash
   # Stop services, drop database, delete archive
   sudo systemctl stop spindle-server spindle-worker
   psql -U spindle -c "DROP DATABASE spindle;"
   rm -rf /var/lib/spindle/archive/*
   
   # Recreate database
   psql -c "CREATE DATABASE spindle OWNER spindle;"
   spindle migrate up
   ```

4. **Restore from backup**
   ```bash
   # Extract latest backup and import
   tar xzf /var/backups/spindle-db-*.tar.gz
   psql "$DATABASE_URL" -f <backup-dir>/spindle-full.sql
   
   # Restore manifests
   bash scripts/restore-manifests.sh
   
   # Restore archive files
   bash scripts/restore-archive.sh
   
   # Verify integrity
   spindle archive verify --path /var/lib/spindle/archive/
   ```

5. **Post-restore verification**
   ```bash
   spindle compliance export --report-type control_status_by_node > /tmp/post-restore-export.json
   diff /tmp/pre-backup-export.json /tmp/post-restore-export.json
   # Should be identical
   ```

## Recovery Point Objective (RPO) & Recovery Time Objective (RTO)

| Metric | Target |
|---|---|
| **RPO** (database) | ≤ 1 hour (WAL archiving) |
| **RPO** (manifests) | ≤ 1 hour |
| **RPO** (raw archive) | ≤ 24 hours |
| **RTO** (database restore) | ≤ 2 hours |
| **RTO** (full restore) | ≤ 4 hours |

## Checklist

### Daily
- [ ] Verify backup job ran successfully
- [ ] Check backup file integrity (SHA-256)
- [ ] Verify WAL archive is current

### Weekly
- [ ] Test restore of manifests table
- [ ] Run `spindle archive verify` on latest backup
- [ ] Verify signing key backup is accessible

### Monthly
- [ ] Full disaster recovery drill (wipe → restore → verify)
- [ ] Review retention policy compliance
- [ ] Update backup scripts for new schema changes

## Emergency Procedures

### If database is corrupted but archive is intact

1. Stop all ingest
2. Restore database from backup
3. Restore raw archive from S3/filesystem backup
4. Run `spindle compliance export` to re-derive compliance from archive
5. The compliance pipeline reconstructs node/run/resource state from raw messages

### If archive is lost but database + manifests are intact

1. Archive data can be reconstructed by replaying ingest from the database
2. Manifests verify the chain of custody for the reconstructed data
3. Run: `spindle archive export --week=<week>` to regenerate Parquet archives

### If manifests are lost

1. **CRITICAL**: Do not panic. Manifests are derived from data + signatures.
2. Re-verify each archive set by recomputing SHA-256 hashes of all files
3. Re-sign with the current signing key
4. **Note**: Chain of custody is broken. Any downstream compliance consumers must be notified.
5. **Lesson**: This is why manifests are backed up with 90-day retention, not 30-day.

## Security Notes

- **Never** store production database credentials in scripts — use environment variables or a secrets manager
- **Never** commit backup scripts with hardcoded passwords
- Signing key backups must be stored offline (air-gapped USB drive / HSM)
- All backup files should be encrypted at rest (e.g., using `gpg` or `age`)
- Access to backup storage must be logged and audited

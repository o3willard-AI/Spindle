# Spindle Backup & Restore Procedure

## Overview

Spindle has three critical data domains that must be backed up:

1. **Database** (PostgreSQL) — node state, run metadata, compliance data, identity mappings, tokens, waivers, sessions, signatures, and all other tables in the `public` schema
2. **Raw archive** (filesystem) — daily directories under `/var/lib/spindle/archive/` containing raw ingest event files (`.json.gz`)
3. **Signing keys** (offline storage) — Ed25519 signing key (`signing-key.aes`) required for archive verification and manifest signing

> **⚠️ SIGNING KEYS ARE THE CHAIN OF CUSTODY. BACK THEM UP FIRST.**

Without the signing key, you can restore raw data but cannot verify archive signatures.
With archive data but no signing key, you lose the ability to authenticate chain-of-custody.
Both should be backed up, but **signing keys take priority**.

## Backup Strategy

### Daily backup schedule

| Component | Tool | Frequency | Retention |
|---|---|---|---|
| Database | `pg_dump` + WAL archiving | Daily full, hourly WAL | 30 days |
| Raw archive | `tar` or `rsync` | Daily | 30 days |
| Signing keys | Manual copy (offline) | After rotation | Indefinite |

### 1. Database backup

```bash
#!/bin/bash
# backup-database.sh — Full PostgreSQL backup with WAL archiving for Spindle.
#
# Usage: bash scripts/backup-database.sh
#
# Environment variables:
#   BACKUP_DIR    — backup destination (default: /var/backups/spindle)
#   DATABASE_URL  — PostgreSQL connection string (default: postgresql://spindle:CHANGE_ME@localhost:5432/spindle)
#   WAL_ARCHIVE   — WAL archive directory (default: /var/lib/postgresql/wal_archive)

set -euo pipefail

BACKUP_DIR="${BACKUP_DIR:-/var/backups/spindle}"
TIMESTAMP=$(date -u +"%Y%m%dT%H%M%SZ")
DB_URL="${DATABASE_URL:-postgresql://spindle:CHANGE_ME@localhost:5432/spindle}"
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
    cp -r "$WAL_ARCHIVE" "$BACKUP_DIR/db/$TIMESTAMP/wal-archive/"
fi

# 3. Create backup manifest
cat > "$BACKUP_DIR/db/$TIMESTAMP/backup-manifest.json" <<EOF
{
    "timestamp": "$TIMESTAMP",
    "type": "full-database",
    "db_dump": "spindle-full.sql",
    "has_wal": $([ -d "$BACKUP_DIR/db/$TIMESTAMP/wal-archive" ] && echo true || echo false),
    "spindle_version": "$($INSTALL_PREFIX/bin/spindle --version 2>/dev/null || echo unknown)"
}
EOF

# 4. Compress
tar czf "$BACKUP_DIR/spindle-db-$TIMESTAMP.tar.gz" -C "$BACKUP_DIR/db/$TIMESTAMP" .

echo "Database backup complete: $BACKUP_DIR/spindle-db-$TIMESTAMP.tar.gz"
```

### 2. Raw archive backup

```bash
#!/bin/bash
# backup-archive.sh — Backup raw archive (filesystem).
#
# Usage: bash scripts/backup-archive.sh
#
# Environment variables:
#   BACKUP_DIR    — backup destination (default: /var/backups/spindle)
#   ARCHIVE_DIR   — local archive directory (default: /var/lib/spindle/archive)

set -euo pipefail

BACKUP_DIR="${BACKUP_DIR:-/var/backups/spindle}"
TIMESTAMP=$(date -u +"%Y%m%dT%H%M%SZ")
ARCHIVE_DIR="${ARCHIVE_DIR:-/var/lib/spindle/archive}"

echo "[backup-archive] Starting archive backup at $TIMESTAMP"

mkdir -p "$BACKUP_DIR/archive/$TIMESTAMP"

# Back up from local filesystem (archive lives under /var/lib/spindle/archive/)
if [ -d "$ARCHIVE_DIR" ] && [ "$(ls -A "$ARCHIVE_DIR" 2>/dev/null)" ]; then
    echo "[backup-archive] Backing up from filesystem: $ARCHIVE_DIR"
    rsync -av --delete "$ARCHIVE_DIR/" "$BACKUP_DIR/archive/$TIMESTAMP/raw/"
    
    # Create archive manifest with SHA-256 checksums
    echo "[backup-archive] Generating manifest..."
    cd "$BACKUP_DIR/archive/$TIMESTAMP/raw"
    find . -name '*.json.gz' -exec sha256sum {} \; > "$BACKUP_DIR/archive/$TIMESTAMP/archive-manifest.txt"
    cd -
    
    echo "[backup-archive] Archive backed up successfully"
else
    echo "[backup-archive] WARNING: No archive data found at $ARCHIVE_DIR"
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

> **⚠️ This procedure requires the database dump and archive tar.gz backups created above.**
> If you only have signing key backups (offline storage), you cannot restore data.

### Before restore

1. **Ensure Spindle services are stopped** (if running)
   ```bash
   # On systems using systemd:
   sudo systemctl stop spindle-server spindle-worker
   # On direct binary deployments, kill any running processes:
   pkill -f spindle-server || true
   pkill -f spindle-worker || true
   ```

2. **Document current state**
   ```bash
   /opt/spindle/bin/spindle runs list --output json > /tmp/runs-before-restore.json
   ```

### Restore Steps (in order)

#### Step 1: Restore database (full dump)

```bash
# Option A: Full restore from pg_dump SQL file
psql "$DATABASE_URL" -f /var/backups/spindle/db/20240101T120000Z/spindle-full.sql

# Option B: If you have a tar.gz backup of the SQL dump
tar xzf /var/backups/spindle-db-*.tar.gz -C /tmp/
psql "$DATABASE_URL" -f /tmp/spindle-full.sql
```

Note: This replaces ALL tables in the `public` schema. If you need to preserve
existing data, run this on a new PostgreSQL instance instead.

#### Step 2: Restore raw archive

```bash
# Restore from filesystem backup
rsync -av /var/backups/spindle/archive/20240101T120000Z/raw/ /var/lib/spindle/archive/

# Or if you extracted from tar.gz:
mkdir -p /var/lib/spindle/archive
tar xzf /var/backups/spindle-archive-*.tar.gz -C /var/lib/spindle/archive/
```

The archive directory should now mirror its state at backup time. Archive files use
the `.json.gz` extension but contain plaintext JSON (they are not gzip-compressed).

#### Step 3: Verify integrity

```bash
# Verify archive files are intact using SHA-256 checksums
cd /var/backups/spindle/archive/*/raw
sha256sum -c ../../archive-manifest.txt

# Cross-verify with Python tool (independent verification)
python3 tools/verify_spindle_archive.py \\
    --keys-url http://localhost:8080/keys.json \\
    --archive /var/lib/spindle/archive/2026-08-09

# Check compliance export returns expected data
/opt/spindle/bin/spindle compliance export --report-type control_status_by_node > /tmp/post-restore-export.json
cat /tmp/post-restore-export.json | head -20
```

#### Step 4: Start services

```bash
# On systems using systemd:
sudo systemctl start spindle-server
sudo systemctl start spindle-worker

# On direct binary deployments:
sudo -u spindle /opt/spindle/bin/spindle-server --config /etc/spindle/spindle.toml &
sudo -u spindle /opt/spindle/bin/spindle-worker --config /etc/spindle/spindle.toml &

# Verify health
curl http://localhost:8080/health
# Expected: exit code 0 with {"status":"healthy",...}
```

#### Step 5: Replay and verify

```bash
# Export compliance data post-restore
/opt/spindle/bin/spindle compliance export --report-type control_status_by_node > /tmp/post-restore-export.json

# Compare with pre-restore export (captured before backup)
diff /tmp/pre-backup-export.json /tmp/post-restore-export.json
# Should be identical (same data, same timestamps from archive)
```

## CI Test Procedure (aspirational)

The following automation target is documented for future implementation. Scripts
in `scripts/` provide manual execution; CI integration is planned once core
backfill pipelines stabilize.

**Available manual scripts:**

| Script | Purpose | Status |
|---|---|---|
| `scripts/backup-database.sh` | Full PostgreSQL backup + WAL export | ✅ Exists |
| `scripts/backup-archive.sh` | Raw archive sync (filesystem) | ✅ Exists |
| `scripts/spindle-install.sh` | Air-gap bundle installer | ✅ Exists |
| `scripts/ci-backup-restore-test.sh` | Backup/restore smoke test | ✅ Exists |
| `scripts/deploy-dex.sh` | Dex OIDC provider deployment | ✅ Exists |
| `scripts/minio-init.sh` | MinIO S3-compatible storage init | ✅ Exists |

Note: The restore script at `scripts/restore-spindle.sh` exists but was found to be binary/corrupt.
Manual restore steps in this document should be used instead until a proper text-based restore script is authored.

Planned CI workflow: `.github/workflows/backup-restore-test.yml` — not yet active.

Manual test procedure:

1. **Pre-export** (capture current state before backup)
   ```bash
   /opt/spindle/bin/spindle compliance export --report-type control_status_by_node > /tmp/pre-backup-export.json
   ```

2. **Run backups**
   ```bash
   bash scripts/backup-database.sh
   bash scripts/backup-archive.sh
   # Signing keys backed up separately from offline storage
   ```

3. **Destroy state** (manual disaster recovery drill)
   ```bash
   # Stop services, drop database, delete archive
   sudo systemctl stop spindle-server spindle-worker || pkill -f spindle-server || true
   psql -U spindle -c "DROP DATABASE spindle;"
   rm -rf /var/lib/spindle/archive/*
   
   # Recreate database (spindle-migrate handles schema creation)
   psql -c "CREATE DATABASE spindle OWNER spindle;"
   /opt/spindle/bin/spindle migrate
   ```

4. **Restore from backup**
   ```bash
   # Extract latest backup and import
   tar xzf /var/backups/spindle-db-*.tar.gz -C /tmp/
   psql "$DATABASE_URL" -f /tmp/spindle-full.sql
   
   # Restore archive files
   mkdir -p /var/lib/spindle/archive
   tar xzf /var/backups/spindle-archive-*.tar.gz -C /var/lib/spindle/archive/
   
   # Verify integrity
   sha256sum -c /var/backups/spindle/archive/*/raw/../../archive-manifest.txt
   ```

5. **Post-restore verification**
   ```bash
   /opt/spindle/bin/spindle compliance export --report-type control_status_by_node > /tmp/post-restore-export.json
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

### If archive is lost but database is intact

1. The database contains all ingested data and computed state
2. Raw archive files can be regenerated by replaying ingest messages from database rows
3. Run compliance export to verify restored compliance data matches expectations
4. New archive files will be created as fresh ingest continues
5. **Note**: Historical chain-of-custody verification requires the signing key; without it, you cannot re-sign historical archives

### If signing keys are lost

1. **CRITICAL**: You can still read restored data and run compliance exports
2. However, you can no longer sign new archives or verify historical signatures
3. Any downstream consumers that verify archive signatures will fail
4. Generate a new signing key and distribute to all consumers
5. **Lesson**: Always maintain offline signing key backups (this is why they take priority)

## Security Notes

- **Never** store production database credentials in scripts — use environment variables or a secrets manager
- **Never** commit backup scripts with hardcoded passwords
- Signing key backups must be stored offline (air-gapped USB drive / HSM)
- All backup files should be encrypted at rest (e.g., using `gpg` or `age`)
- Access to backup storage must be logged and audited

# Spindle Operator Rollback Procedure

> **Target audience:** on-call operators, SREs, and deploy engineers.
> **Goal:** Anyone reading this can execute a rollback without asking questions.
> Every command below is copy-paste ready.

## Prerequisites

- SSH access to the Spindle host(s)
- `spindle --version` output from the **current** (broken) deployment
- Access to backup storage at `/var/backups/spindle/`
- `SPINDLE_PRODUCTION=1` must be set in the production environment
- Root or sudo access to run `systemctl`, `psql`, and file operations

---

## 1. Migration Rollback

### When to use

A migration introduced a data-corrupting schema change or the new query
paths are broken. You need to revert the migration.

### Step 1: Identify the migration and current version

```bash
# Check current migration version
psql "$DATABASE_URL" -t -c "SELECT version FROM spindle_migration_version ORDER BY version DESC LIMIT 1;"

# List all applied migrations
psql "$DATABASE_URL" -t -c "SELECT version, name FROM spindle_migrations ORDER BY version;"

# List migrations on disk
ls -1 migrations/
```

### Step 2: Check if the migration has a `down.sql`

```bash
MIGRATION_DIR="migrations/018_extra_fields"
ls "$MIGRATION_DIR/down.sql"  # Does it exist?
```

**If `down.sql` exists:** apply it directly:

```bash
# Apply the down migration
psql "$DATABASE_URL" -f "$MIGRATION_DIR/down.sql"

# Mark the version as rolled back
psql "$DATABASE_URL" -c "DELETE FROM spindle_migrations WHERE version = '018_extra_fields';"

# Restart so workers pick up the schema
sudo systemctl restart spindle-server spindle-worker
```

### Step 3: For destructive migrations (no `down.sql` or DROP+recreate)

Most Spindle migrations are **forward-only** (ALTER TABLE ADD COLUMN, etc.).
There is no automatic `down.sql` for destructive migrations (DROP TABLE,
DROP COLUMN, schema rebuilds like migration 020). Roll back via **backup-restore**:

```bash
# 1. Stop all Spindle services
sudo systemctl stop spindle-server spindle-worker

# 2. Restore database from the backup taken BEFORE the migration
#    Find the latest backup before the migration date
ls -lt /var/backups/spindle/db/*/ | head -10

# 3. Restore the full database dump
#    Replace TIMESTAMP with the backup you want to restore
pg_restore --clean --if-exists -d "$DATABASE_URL" \
  /var/backups/spindle/db/20240101T120000Z/spindle-full.dump

# 4. Re-apply only the migrations you want (skip the broken one)
cd /home/operator/workspace/Spindle
cargo run -p spindle-migrate -- up --target 018

# 5. Start services
sudo systemctl start spindle-server spindle-worker
```

### Step 4: Verify migration rollback

```bash
# Confirm the schema is back
psql "$DATABASE_URL" -t -c "SELECT column_name FROM information_schema.columns WHERE table_name = 'nodes' AND column_name = 'attributes';"
# Should return nothing if migration 020 was rolled back

# Check health
curl -sf http://localhost:8080/health || echo "HEALTH CHECK FAILED"
```

---

## 2. Deployment Rollback

### When to use

A new binary build introduced a runtime crash, incorrect data handling,
or a configuration error. Roll back to the previous known-good version.

### Step 1: Identify the current and target versions

```bash
# Current version (broken)
/opt/spindle/bin/spindle-server --version
# Example output: spindle-server 0.1.0 (git: abc12345, built: epoch-1786489011)

# Find the previous known-good commit/SHA
# Check the deployment log or git history
git log --oneline -10
# Or check the installed version
cat /opt/spindle/VERSION
```

### Step 2: Stop the current deployment

```bash
# Stop services
sudo systemctl stop spindle-server spindle-worker

# Verify they're stopped
sudo systemctl status spindle-server  # should show "inactive"
pgrep -f spindle-server && echo "STILL RUNNING" || echo "Stopped OK"
```

### Step 3: Deploy the previous binary

**For air-gap installations** (using a pre-built bundle):

```bash
# Find the previous bundle
ls -1 /opt/spindle/bundles/spindle-bundle-*.tar.gz

# Extract the previous version (adjust the path/timestamp)
cd /tmp
tar xzf /opt/spindle/bundles/spindle-bundle-v0.0.9.tar.gz

# Install
sudo -u spindle /tmp/spindle-bundle/install.sh --prefix /opt/spindle

# Verify version
/opt/spindle/bin/spindle-server --version
# Should show the old git SHA
```

**For source builds** (development or local deployments):

```bash
# Check out the previous commit
cd /home/operator/workspace/Spindle
git log --oneline -10

# Revert to the previous known-good version
# Replace <GOOD_COMMIT> with the SHA from Step 1
git checkout <GOOD_COMMIT>

# Rebuild
cargo build --release -p spindle-server
cargo build --release -p spindle-worker

# Copy binaries
sudo cp target/release/spindle-server /opt/spindle/bin/
sudo cp target/release/spindle-worker /opt/spindle/bin/
sudo chown spindle:spindle /opt/spindle/bin/spindle-server /opt/spindle/bin/spindle-worker

# Return to main branch
git checkout main

# Verify
/opt/spindle/bin/spindle-server --version
```

### Step 4: Restore database (if the bad deployment corrupted data)

```bash
# Only if the new deployment wrote corrupt data
pg_restore --clean --if-exists -d "$DATABASE_URL" \
  /var/backups/spindle/db/20240101T120000Z/spindle-full.dump

# Restore archive if needed
rsync -av /var/backups/spindle/archive/20240101T120000Z/raw/ /var/lib/spindle/archive/
```

### Step 5: Start services and verify

```bash
sudo systemctl start spindle-server
sleep 3  # Give it time to start

# Verify health
curl -sf http://localhost:8080/health
# Expected: exit code 0, JSON with all subsystems "status":"up"

# Verify the version
/opt/spindle/bin/spindle-server --version

# Check logs for errors
sudo journalctl -u spindle-server --since "5 minutes ago" -n 50
```

---

## 3. Database Restore from Archive

### When to use

The database is corrupted, dropped, or otherwise unusable. You have a
`pg_dump` backup and/or WAL archive. Restore to the latest consistent state.

### Step 1: Stop all Spindle services

```bash
sudo systemctl stop spindle-server spindle-worker
pkill -f spindle-server || true
pkill -f spindle-worker || true
```

### Step 2: Identify the backup to restore

```bash
# List all database backups
ls -lt /var/backups/spindle/db/*/

# Pick the most recent one that's consistent (before corruption)
# Backup directories are named by timestamp
ls -1 /var/backups/spindle/db/
# Example: 20240101T120000Z/
```

### Step 3: Restore the database

```bash
# Option A: From SQL dump (fastest for small databases)
psql "$DATABASE_URL" -f /var/backups/spindle/db/20240101T120000Z/spindle-full.sql

# Option B: From pg_dump (preserves more metadata)
pg_restore --clean --if-exists -d "$DATABASE_URL" \
  /var/backups/spindle/db/20240101T120000Z/spindle-full.dump

# Option C: From tar.gz backup (extract first)
mkdir -p /tmp/spindle-restore
tar xzf /var/backups/spindle-db-20240101T120000Z.tar.gz -C /tmp/spindle-restore/
psql "$DATABASE_URL" -f /tmp/spindle-restore/spindle-full.sql

# For point-in-time recovery (PITR) — only if WAL archive is available
# Restore base backup, then replay WAL to a specific time
pg_basebackup -D /var/lib/postgresql/data/restore -Ft -z -P -U spindle
# ... then replay WAL segments up to the desired point
```

### Step 4: Restore the raw archive (if also lost)

```bash
# The raw archive is separate from the database
rsync -av /var/backups/spindle/archive/20240101T120000Z/raw/ /var/lib/spindle/archive/

# Verify archive file integrity
cd /var/backups/spindle/archive/20240101T120000Z/raw/
sha256sum -c ../archive-manifest.txt
```

### Step 5: Restore signing key (if also lost)

```bash
# Copy signing key from offline backup storage
sudo cp /mnt/offline-storage/spindle-key-*.aes /opt/spindle/signing-key.aes
sudo chown spindle:spindle /opt/spindle/signing-key.aes
sudo chmod 600 /opt/spindle/signing-key.aes
```

### Step 6: Start services and verify

```bash
sudo systemctl start spindle-server
sleep 5

# Verify database connectivity
/opt/spindle/bin/spindle-server --validate-config

# Verify health endpoint
curl -sf http://localhost:8080/health

# Verify data integrity
psql "$DATABASE_URL" -t -c "SELECT COUNT(*) FROM nodes;"
psql "$DATABASE_URL" -t -c "SELECT COUNT(*) FROM runs;"
psql "$DATABASE_URL" -t -c "SELECT COUNT(*) FROM compliance_reports;"
```

---

## Post-Rollback Verification Checklist

Run ALL of these after any rollback scenario:

```bash
# 1. Service status
sudo systemctl status spindle-server --no-pager
sudo systemctl status spindle-worker --no-pager

# 2. Health endpoint (must return 200 with all subsystems up)
curl -sf http://localhost:8080/health | python3 -m json.tool

# 3. Version check (should show old git SHA)
/opt/spindle/bin/spindle-server --version

# 4. Database connectivity
psql "$DATABASE_URL" -c "SELECT 1;"

# 5. Key tables have data
psql "$DATABASE_URL" -t -c "
  SELECT 'nodes', COUNT(*) FROM nodes
  UNION ALL SELECT 'runs', COUNT(*) FROM runs
  UNION ALL SELECT 'waivers', COUNT(*) FROM waivers;
"

# 6. API endpoints respond
curl -sf -H "Authorization: Bearer $SPINDLE_INGEST_TOKEN" \
  http://localhost:8080/v1/nodes | python3 -m json.tool

# 7. Archive is accessible
/opt/spindle/bin/spindle archive list --limit 5

# 8. Check for errors in logs
sudo journalctl -u spindle-server --since "10 minutes ago" -p err --no-pager
```

If any check fails, do **not** mark the rollback as complete. Investigate and
fix before returning the system to service.

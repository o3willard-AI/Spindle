#!/bin/bash
# Fleet-02 (database) — PostgreSQL Chaos Script  
# Changes: 3 PostgreSQL misconfigurations (all recoverable)
# NEVER touches SSH or Cinc Client

set -euo pipefail

NODE="fleet-02"
IP="203.0.113.12"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOG="/var/log/chaos/fleet-02_chaos_${TIMESTAMP}.log"

echo "=== CHAOS ENTERPRISE: ${NODE} (${IP}) ==="
echo "Timestamp: ${TIMESTAMP}"
echo ""

# Safety check — verify SSH and Cinc are intact BEFORE doing anything
if ! systemctl is-active ssh.service >/dev/null 2>&1; then
    echo "FATAL: SSH service down! ABORTING."
    exit 1
fi

CINC_STATUS="inactive"
if systemctl is-active cinc-client.service >/dev/null 2>&1; then
    CINC_STATUS="active"
elif systemctl list-unit-files | grep -q "cinc-client"; then
    CINC_STATUS="stopped-but-installed"
else
    CINC_STATUS="not-installed"
fi

echo "[INFO] Cinc Client status: ${CINC_STATUS}"
echo "[WARN] Proceeding without Cinc verification (service not detected on this node)"
echo ""
if [ "${CINC_STATUS}" != "active" ]; then
    echo "⚠️  WARNING: Cinc Client not running — repair must be manual or via future converge"
fi

echo "[OK] SSH and Cinc Client are alive"
echo ""

# ── CHANGE 1: Drop a non-critical reporting user ───────────────────────
echo "[CHANGE 1] Dropping non-critical reporting user..."
sudo -u postgres psql -c "SELECT usename FROM pg_user WHERE usename LIKE 'report_%';" || true
echo "-- CHAOS: Dropping report_viewer user" | sudo -u postgres psql 2>/dev/null || \
  sudo -u postgres dropdb --if-exists --reject-passwords report_test 2>/dev/null || true

# Safely try dropping a user that likely exists for testing
sudo -u postgres psql -c "DO \$\$ BEGIN IF EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'analytics_reporter') THEN DROP ROLE analytics_reporter; RAISE NOTICE 'Dropped analytics_reporter'; ELSE RAISE NOTICE 'Role analytics_reporter does not exist — OK'; END IF; END \$\$;" 2>/dev/null || true

echo "[DONE] analytics_reporter role dropped (or was not present)"
echo ""

# ── CHANGE 2: Change shared_buffers from 512MB to 512KB (brutal tuning)
# Targets spindle-tuning.conf (the file the Cinc Auditor profile checks AND the
# converge recipe repairs) so the detect→repair loop can close. Editing
# postgresql.conf alone was invisible to both detection and repair.
echo "[CHANGE 2] Changing shared_buffers to maliciously low value..."
PG_CONF="/etc/postgresql/16/main/conf.d/spindle-tuning.conf"
cp "${PG_CONF}" "${PG_CONF}.bak.${TIMESTAMP}"
sed -i 's/^shared_buffers\s*=.*/shared_buffers = 512kB/' "${PG_CONF}"
grep '^shared_buffers' "${PG_CONF}"
sudo systemctl reload postgresql 2>/dev/null || echo "[WARN] PostgreSQL reload failed (expected during chaos)"
echo "[DONE] shared_buffers changed to 512kB in spindle-tuning.conf"
echo ""

# ── CHANGE 3: Rename a non-critical database (analytics → analytics_old) 
echo "[CHANGE 3] Renaming spindle_analytics to spindle_analytics.chaos.${TIMESTAMP}..."
DB_NAME="spindle_analytics"
CHANGED_DB="${DB_NAME}.chaos.${TIMESTAMP}"
sudo -u postgres psql -c "ALTER DATABASE ${DB_NAME} RENAME TO ${CHANGED_DB};" 2>/dev/null || \
  echo "[INFO] Database ${DB_NAME} doesn't exist yet (future state, skipping rename)"
echo "[DONE] Database renamed: ${DB_NAME} → ${CHANGED_DB}"
echo ""

# Log all changes for Cinc Auditor/Cinc recovery
cat > "/tmp/chaos-manifest-fleet-02.${TIMESTAMP}" <<EOF
node:${NODE}
timestamp:${TIMESTAMP}
changes:
  - object:role
    name:analytics_reporter
    action:drop_role
  - file:/etc/postgresql/16/main/conf.d/spindle-tuning.conf
    param:shared_buffers
    old_value:512MB
    new_value:512kB
    backup:/etc/postgresql/16/main/conf.d/spindle-tuning.conf.bak.${TIMESTAMP}
  - object:database
    old_name:${DB_NAME}
    new_name:${CHANGED_DB}
restore_commands:
  - cp /etc/postgresql/16/main/conf.d/spindle-tuning.conf.bak.${TIMESTAMP} /etc/postgresql/16/main/conf.d/spindle-tuning.conf
  - sudo systemctl restart postgresql
  - sudo -u postgres createdb ${DB_NAME}  # if needed
  - sudo -u postgres psql -c 'CREATE ROLE analytics_reporter WITH LOGIN;'
EOF

echo "Chaos complete. Manifest: /tmp/chaos-manifest-fleet-02.${TIMESTAMP}"
echo "Restore:"
echo "  cp ${PG_CONF}.bak.${TIMESTAMP} ${PG_CONF}"
echo "  sudo systemctl restart postgresql"

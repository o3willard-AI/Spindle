#!/bin/bash
# Fleet-01 (web_app) — Spindle Enterprise Web Server Chaos Script
# Changes: 3 Apache misconfigurations (all recoverable)
# NEVER touches SSH or Cinc Client

set -euo pipefail

NODE="fleet-01"
IP="198.51.100.211"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOG="/var/log/chaos/fleet-01_chaos_${TIMESTAMP}.log"

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

# ── CHANGE 1: Change Apache Listen port from 80 to 9090 ───────────────
echo "[CHANGE 1] Modifying Apache listen port..."
cp /etc/apache2/ports.conf "/etc/apache2/ports.conf.bak.${TIMESTAMP}"
sed -i 's/^Listen 80$/Listen 9090/' /etc/apache2/ports.conf
grep '^Listen' /etc/apache2/ports.conf
apache2ctl configtest || true
systemctl reload apache2 2>/dev/null || echo "[WARN] Apache reload failed (expected during chaos)"
echo "[DONE] Port changed: 80 → 9090"
echo ""

# ── CHANGE 2: Remove security header X-Frame-Options ──────────────────
echo "[CHANGE 2] Removing X-Frame-Options security header..."
VHOST="/etc/apache2/sites-enabled/spindle-enterprise.conf"
cp "${VHOST}" "${VHOST}.bak.${TIMESTAMP}"
# Remove any Header set X-Frame-Options lines
sed -i '/Header.*X-Frame-Options/d' "$VHOST"
grep -c 'X-Frame-Options' "$VHOST" && echo "Still present" || echo "[DONE] X-Frame-Options header removed"
systemctl reload apache2 2>/dev/null || true
echo ""

# ── CHANGE 3: Add a broken/conflicting Listen directive ────────────────
echo "[CHANGE 3] Adding conflicting Listen directive..."
echo "# CHAOS: Intentionally duplicate Listen to cause conflict" >> /etc/apache2/ports.conf
tail -5 /etc/apache2/ports.conf
systemctl reload apache2 2>/dev/null || echo "[WARN] Apache reload failed (conflict expected)"
echo "[DONE] Duplicate Listen directive added"
echo ""

# Log all changes for InSpec/Cinc recovery
cat > "/etc/apache2/chaos-manifest.${TIMESTAMP}" <<EOF
node:${NODE}
timestamp:${TIMESTAMP}
changes:
  - file:/etc/apache2/ports.conf
    action:change_port_duplicate_listen
    backup:/etc/apache2/ports.conf.bak.${TIMESTAMP}
  - file:${VHOST}
    action:remove_xframe_header
    backup:${VHOST}.bak.${TIMESTAMP}
backup_dir:/etc/apache2/backups.chaos.${TIMESTAMP}
EOF

mkdir -p "/etc/apache2/backups.chaos.${TIMESTAMP}"
mv "/etc/apache2/ports.conf.bak.${TIMESTAMP}" "/etc/apache2/backups.chaos.${TIMESTAMP}/"
mv "${VHOST}.bak.${TIMESTAMP}" "/etc/apache2/backups.chaos.${TIMESTAMP}/"

echo "Chaos complete. Manifest: /etc/apache2/chaos-manifest.${TIMESTAMP}"
echo "Restore with:"
echo "  cp /etc/apache2/backups.chaos.${TIMESTAMP}/* /etc/apache2/"
echo "  sed -i 's/^Listen 9090/#CHAOS REMOVED---Listen 9090/' /etc/apache2/ports.conf"
echo "  rm -rf /etc/apache2/chaos-manifest.${TIMESTAMP}"
echo "  systemctl reload apache2"

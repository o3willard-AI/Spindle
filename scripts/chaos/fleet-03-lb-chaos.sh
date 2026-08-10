#!/bin/bash
# Fleet-03 (loadbalancer) — HAProxy Chaos Script  
# Changes: 3 HAProxy misconfigurations (all recoverable)
# NEVER touches SSH or Cinc Client

set -euo pipefail

NODE="fleet-03"
IP="198.51.100.213"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOG="/var/log/chaos/fleet-03_chaos_${TIMESTAMP}.log"

echo "=== CHAOS ENTERPRISE: ${NODE} (${IP}) ==="
echo "Timestamp: ${TIMESTAMP}"
echo ""

# Safety check — verify SSH and Cinc are intact BEFORE doing anything
if ! systemctl is-active ssh.service >/dev/null 2>&1; then
    echo "FATAL: SSH service down! ABORTING."
    exit 1
fi

if ! systemctl is-active cinc-client.service >/dev/null 2>&1; then
    echo "FATAL: Cinc Client service down! ABORTING."
    exit 1
fi

echo "[OK] SSH and Cinc Client are alive"
echo ""

# ── CHANGE 1: Point backend pool to dead IP ────────────────────────────
echo "[CHANGE 1] Adding dead server to backend pool..."
HAPROXY_CFG="/etc/haproxy/haproxy.cfg"
cp "${HAPROXY_CFG}" "${HAPROXY_CFG}.bak.${TIMESTAMP}"

# Find a frontend/backend section and add a dead server
BACKEND_NAME="webservers"
sed -i "/^\s*balance.*roundrobin/a\\    server fleet-03-dead 203.0.113.1:8080 maxconn 1 check" "$HAPROXY_CFG"

echo "Added dead server: 203.0.113.1:8080"
haproxy -c -f "$HAPROXY_CFG" 2>/dev/null || echo "[WARN] Config validation failed (dead server expected)"
systemctl reload haproxy 2>/dev/null || echo "[WARN] HAProxy reload failed (expected during chaos)"
echo "[DONE] Dead backend server added"
echo ""

# ── CHANGE 2: Change health check interval from 2s to 60s (broken detection) 
echo "[CHANGE 2] Changing health check interval to 60s..."
# This affects all existing servers' health checks
sed -i 's/check inter 2s/check inter 60s/' "$HAPROXY_CFG"
grep 'check inter' "$HAPROXY_CFG" | head -5
systemctl reload haproxy 2>/dev/null || true
echo "[DONE] Health check interval changed: 2s → 60s"
echo ""

# ── CHANGE 3: Add excessive timeout that will cause connection drops ──
echo "[CHANGE 3] Setting dangerously low client timeout..."
sed -i 's/timeout client 30s/timeout client 2s/' "$HAPROXY_CFG"
grep 'timeout client' "$HAPROXY_CFG"
systemctl reload haproxy 2>/dev/null || true
echo "[DONE] Client timeout changed: 30s → 2s"
echo ""

# Log all changes for InSpec/Cinc recovery
cat > "/tmp/chaos-manifest-fleet-03.${TIMESTAMP}" <<EOF
node:${NODE}
timestamp:${TIMESTAMP}
changes:
  - file:/etc/haproxy/haproxy.cfg
    action:add_dead_server
    detail:server fleet-03-dead 203.0.113.1:8080
  - param:health_check_interval
    old_value:2s
    new_value:60s
  - param:client_timeout
    old_value:30s
    new_value:2s
backup:/etc/haproxy/haproxy.cfg.bak.${TIMESTAMP}
restore_commands:
  - cp ${HAPROXY_CFG}.bak.${TIMESTAMP} ${HAPROXY_CFG}
  - systemctl restart haproxy
EOF

echo "Chaos complete. Manifest: /tmp/chaos-manifest-fleet-03.${TIMESTAMP}"
echo "Restore:"
echo "  cp ${HAPROXY_CFG}.bak.${TIMESTAMP} ${HAPROXY_CFG}"
echo "  systemctl restart haproxy"

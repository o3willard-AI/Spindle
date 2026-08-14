#!/bin/bash
# chaos-port-shift.sh — Drift type 6: port-shift
# Rewrites the listen port in the app config → fails the http(...) control
#
# Fails: http(...) check (e.g. web-01 expects port 80; lb-02 expects port 443)
# Repair: cinc-client --once (chef template rewrites config + restarts)
#
# Usage: chaos-port-shift.sh <target_node> <app>

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../library/chaos_safety.sh"

TARGET_NODE="${1:-}"
TARGET_APP="${2:-}"

if [ -z "$TARGET_NODE" ] || [ -z "$TARGET_APP" ]; then
    echo "Usage: $0 <target_node> <app>"
    exit 1
fi

chaos_init "port-shift" "$TARGET_APP" "$TARGET_NODE" || {
    chaos_log "FATAL" "port-shift: pre-flight checks failed"
    exit 1
}

chaos_log "APPLY" "port-shift: rewriting listen port on ${CHAOS_NODE} (${CHAOS_ROLE})"

# ── Apply drift: change the listen port in the app config ───────────────────
# Backup the config file before mutating
chaos_backup_file "$CHAOS_CONFIG" "pre_port_shift"

# Use a random high port to ensure it's not in use
CHAOSED_PORT=$((8080 + (RANDOM % 8000)))
# Ensure it's not the same as the original
case "$CHAOS_ROLE" in
    web)     ORIGINAL_PORT=80 ;;
    loadbalancer) ORIGINAL_PORT=443 ;;
    database) ORIGINAL_PORT=5432 ;;
    *)       ORIGINAL_PORT=80 ;;
esac

chaos_log "INFO" "Shifting port: $ORIGINAL_PORT → $CHAOSED_PORT"

case "$CHAOS_ROLE" in
    web)
        # Apache: change all occurrences of ':80' and 'Listen 80' to the chaos port
        sed -i "s/^Listen ${ORIGINAL_PORT}/Listen ${CHAOSED_PORT}/g" "$CHAOS_CONFIG"
        # Also patch the vhost to listen on the new port
        for vhost in /etc/apache2/sites-enabled/spindle-enterprise.conf /etc/apache2/sites-enabled/freshrss.conf /etc/apache2/sites-enabled/rss-bridge.conf; do
            if [ -f "$vhost" ]; then
                sed -i "s/:${ORIGINAL_PORT}/:${CHAOSED_PORT}/g" "$vhost"
                chaos_backup_file "$vhost" "pre_port_shift_vhost"
            fi
        done
        # Reload to apply (may fail — that's expected during chaos)
        systemctl reload apache2 2>/dev/null || chaos_log "WARN" "Apache reload failed (expected during chaos)"
        ;;
    loadbalancer)
        # HAProxy: change SSL incoming port in haproxy.cfg
        sed -i "s/bind \*:${ORIGINAL_PORT}/bind \*:${CHAOSED_PORT}/g" "$CHAOS_CONFIG"
        # Template render may use different format; also try 'ssl_incoming_port' style
        sed -i "s/bind \*:ssl_incoming_port/bind \*:${CHAOSED_PORT}/g" "$CHAOS_CONFIG" 2>/dev/null || true
        haproxy -c -f "$CHAOS_CONFIG" 2>/dev/null || chaos_log "WARN" "HAProxy config check failed (expected during chaos)"
        systemctl reload haproxy 2>/dev/null || chaos_log "WARN" "HAProxy reload failed (expected during chaos)"
        ;;
    database)
        # PostgreSQL: change listen_addresses port in postgresql.conf
        local_pg_conf="/etc/postgresql/16/main/postgresql.conf"
        if [ -f "$local_pg_conf" ]; then
            chaos_backup_file "$local_pg_conf" "pre_port_shift_pg"
            sed -i "s/^port = ${ORIGINAL_PORT}/port = ${CHAOSED_PORT}/g" "$local_pg_conf"
            systemctl reload postgresql 2>/dev/null || chaos_log "WARN" "PostgreSQL reload failed"
        fi
        ;;
esac

chaos_log "DRIFT" "Port shifted: ${ORIGINAL_PORT} → ${CHAOSED_PORT} on ${CHAOS_NODE}"

# Track port restoration for emergency revert
CHAOSED_PORT_LOCAL="$CHAOSED_PORT"
ORIGINAL_PORT_LOCAL="$ORIGINAL_PORT"
chaos_track_command "restore_port" "cp '${CHAOS_BACKUP_DIR}/$(basename "$CHAOS_CONFIG").bak_${CHAOS_TIMESTAMP}' '$CHAOS_CONFIG' && systemctl reload ${CHAOS_SERVICE} 2>/dev/null || true"

# ── Post-check ──────────────────────────────────────────────────────────────
if ! chaos_finalize; then
    chaos_log "FATAL" "port-shift: safety guard tripped — auto-reverted"
    exit 1
fi

exit 0

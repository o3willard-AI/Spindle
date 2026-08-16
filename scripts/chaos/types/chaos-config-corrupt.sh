#!/bin/bash
# chaos-config-corrupt.sh — Drift type 7: config-corrupt
# Injects a bad directive or truncates config → fails fleet-services + misconfig
#
# Fails: fleet-services + misconfig (role Cinc Auditor controls)
# Repair: cinc-client --once (chef template rewrites config + restarts)
#
# Usage: chaos-config-corrupt.sh <target_node> <app>

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../library/chaos_safety.sh"

TARGET_NODE="${1:-}"
TARGET_APP="${2:-}"

if [ -z "$TARGET_NODE" ] || [ -z "$TARGET_APP" ]; then
    echo "Usage: $0 <target_node> <app>"
    exit 1
fi

chaos_init "config-corrupt" "$TARGET_APP" "$TARGET_NODE" || {
    chaos_log "FATAL" "config-corrupt: pre-flight checks failed"
    exit 1
}

chaos_log "APPLY" "config-corrupt: injecting bad directive into ${CHAOS_CONFIG} on ${CHAOS_NODE}"

# ── Apply drift: corrupt the app config ─────────────────────────────────────
# Backup the config file before corrupting
chaos_backup_file "$CHAOS_CONFIG" "pre_config_corruption"

case "$CHAOS_ROLE" in
    web)
        # Truncate the vhost config to remove DocumentRoot/ServerName directives
        VHOST="/etc/apache2/sites-enabled/spindle-enterprise.conf"
        if [ -f "$VHOST" ]; then
            chaos_backup_file "$VHOST" "pre_config_corruption_vhost"
            # Remove DocumentRoot and ServerName directives — breaks the vhost
            sed -i '/DocumentRoot/d' "$VHOST"
            sed -i '/ServerName/d' "$VHOST"
            # Inject a malformed directive
            echo "# CHAOS-BAD-DIRECTIVE {{{{" >> "$VHOST"
            echo "InvalidDirective ThatShouldNotParse <{" >> "$VHOST"
        fi
        ;;
    loadbalancer)
        # Inject a malformed backend and truncate a frontend section
        # Add a backend with impossible syntax
        echo "" >> "$CHAOS_CONFIG"
        echo "# CHAOS: malformed backend" >> "$CHAOS_CONFIG"
        echo "backend chaos-corrupted-backend" >> "$CHAOS_CONFIG"
        echo "    mode http" >> "$CHAOS_CONFIG"
        echo "    server bad-server 999.999.999.999:99999 check" >> "$CHAOS_CONFIG"
        echo "    BALANCE BROKEN SYNTAX {{{" >> "$CHAOS_CONFIG"
        ;;
    database)
        # Truncate the tuning config and inject bad parameter
        PG_CONF="/etc/postgresql/16/main/conf.d/spindle-tuning.conf"
        if [ -f "$PG_CONF" ]; then
            chaos_backup_file "$PG_CONF" "pre_config_corruption_tuning"
            # Truncate to empty + inject garbage
            > "$PG_CONF"
            echo "# CHAOS: corrupted tuning config" >> "$PG_CONF"
            echo "this_is_not_valid_postgresql_config = true {" >> "$PG_CONF"
        fi
        ;;
esac

# Attempt a reload (expected to fail — that's the point of config drift)
case "$CHAOS_ROLE" in
    web)        systemctl reload apache2 2>/dev/null || chaos_log "WARN" "Apache reload failed (expected — config is corrupt)" ;;
    loadbalancer) haproxy -c -f "$CHAOS_CONFIG" 2>/dev/null || chaos_log "WARN" "HAProxy config check failed (expected — config is corrupt)" ;;
    database)   systemctl reload postgresql 2>/dev/null || chaos_log "WARN" "PostgreSQL reload failed (expected — config is corrupt)" ;;
esac

chaos_log "DRIFT" "Config $CHAOS_CONFIG corrupted with bad directives on ${CHAOS_NODE}"

# Track restoration for emergency revert
chaos_track_command "restore_config" "cp '${CHAOS_BACKUP_DIR}/$(basename "$CHAOS_CONFIG").bak_${CHAOS_TIMESTAMP}' '$CHAOS_CONFIG' && systemctl reload ${CHAOS_SERVICE} 2>/dev/null || true"

# ── Post-check ──────────────────────────────────────────────────────────────
if ! chaos_finalize; then
    chaos_log "FATAL" "config-corrupt: safety guard tripped — auto-reverted"
    exit 1
fi

exit 0

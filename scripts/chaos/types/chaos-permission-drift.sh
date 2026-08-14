#!/bin/bash
# chaos-permission-drift.sh — Drift type 8: permission-drift
# Changes ownership/mode on a managed config file → fails file/perm control
#
# Fails: file/perm control (role InSpec controls)
# Repair: cinc-client --once (chef file resource enforces mode + owner)
#
# Usage: chaos-permission-drift.sh <target_node> <app>

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../library/chaos_safety.sh"

TARGET_NODE="${1:-}"
TARGET_APP="${2:-}"

if [ -z "$TARGET_NODE" ] || [ -z "$TARGET_APP" ]; then
    echo "Usage: $0 <target_node> <app>"
    exit 1
fi

chaos_init "permission-drift" "$TARGET_APP" "$TARGET_NODE" || {
    chaos_log "FATAL" "permission-drift: pre-flight checks failed"
    exit 1
}

chaos_log "APPLY" "permission-drift: changing ownership/mode of ${CHAOS_CONFIG} on ${CHAOS_NODE}"

# ── Apply drift: chmod/chown the managed config file ──────────────────────────
# Backup is implicit (we track the original mode/owner for revert)
ORIG_MODE=$(stat -c '%a' "$CHAOS_CONFIG" 2>/dev/null || echo "0644")
ORIG_OWNER=$(stat -c '%U:%G' "$CHAOS_CONFIG" 2>/dev/null || echo "root:root")
ORIG_PERMS="${ORIG_MODE} ${ORIG_OWNER}"

chaos_log "INFO" "Original perms: ${ORIG_PERMS}"

# Apply malicious drift: make it world-writable + change ownership
chmod 0777 "$CHAOS_CONFIG" 2>/dev/null || chaos_log "WARN" "chmod failed on $CHAOS_CONFIG"
chown 0:0 "$CHAOS_CONFIG" 2>/dev/null || chaos_log "WARN" "chown failed on $CHAOS_CONFIG"

chaos_log "DRIFT" "Permissions corrupted: ${ORIG_PERMS} → 0777 root:root on ${CHAOS_CONFIG}"

# For role-specific files, also drift a secondary managed file
case "$CHAOS_ROLE" in
    web)
        SECONDARY="/etc/apache2/conf-available/security-headers.conf"
        if [ -f "$SECONDARY" ]; then
            chaos_backup_file "$SECONDARY" "pre_perm_drift_secondary"
            chmod 0666 "$SECONDARY" 2>/dev/null || true
            chown nobody:nogroup "$SECONDARY" 2>/dev/null || true
            chaos_log "DRIFT" "Secondary file $SECONDARY also corrupted"
            chaos_track_command "restore_secondary_perms" "cp '${CHAOS_BACKUP_DIR}/$(basename "$SECONDARY").bak_${CHAOS_TIMESTAMP}' '$SECONDARY'"
        fi
        ;;
    loadbalancer)
        SECONDARY="/etc/haproxy/ssl/spindle.pem"
        if [ -f "$SECONDARY" ]; then
            chaos_backup_file "$SECONDARY" "pre_perm_drift_secondary"
            chmod 0666 "$SECONDARY" 2>/dev/null || true
            chown nobody:nogroup "$SECONDARY" 2>/dev/null || true
            chaos_log "DRIFT" "Secondary file $SECONDARY also corrupted"
            chaos_track_command "restore_secondary_perms" "cp '${CHAOS_BACKUP_DIR}/$(basename "$SECONDARY").bak_${CHAOS_TIMESTAMP}' '$SECONDARY'"
        fi
        ;;
    database)
        SECONDARY="/etc/postgresql/16/main/conf.d/spindle-tuning.conf"
        if [ -f "$SECONDARY" ]; then
            chaos_backup_file "$SECONDARY" "pre_perm_drift_secondary"
            chmod 0666 "$SECONDARY" 2>/dev/null || true
            chown nobody:nogroup "$SECONDARY" 2>/dev/null || true
            chaos_log "DRIFT" "Secondary file $SECONDARY also corrupted"
            chaos_track_command "restore_secondary_perms" "cp '${CHAOS_BACKUP_DIR}/$(basename "$SECONDARY").bak_${CHAOS_TIMESTAMP}' '$SECONDARY'"
        fi
        ;;
esac

# Track restoration for emergency revert
chaos_track_command "restore_permissions" "chmod ${ORIG_MODE} '${CHAOS_CONFIG}' && chown ${ORIG_OWNER} '${CHAOS_CONFIG}'"

# ── Post-check ──────────────────────────────────────────────────────────────
if ! chaos_finalize; then
    chaos_log "FATAL" "permission-drift: safety guard tripped — auto-reverted"
    exit 1
fi

exit 0

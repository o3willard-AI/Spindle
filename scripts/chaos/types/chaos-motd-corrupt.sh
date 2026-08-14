#!/bin/bash
# chaos-motd-corrupt.sh — Drift type 3: motd-corrupt
# Overwrites /etc/motd with garbage → fails motd-1.0
#
# Fails: motd-1.0 (base InSpec profile)
# Repair: cinc-client --once (recipe[base] rewrites /etc/motd)
#
# Usage: chaos-motd-corrupt.sh <target_node> <app>

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../library/chaos_safety.sh"

TARGET_NODE="${1:-}"
TARGET_APP="${2:-}"

if [ -z "$TARGET_NODE" ] || [ -z "$TARGET_APP" ]; then
    echo "Usage: $0 <target_node> <app>"
    exit 1
fi

chaos_init "motd-corrupt" "$TARGET_APP" "$TARGET_NODE" || {
    chaos_log "FATAL" "chaos-motd-corrupt: pre-flight checks failed"
    exit 1
}

chaos_log "APPLY" "motd-corrupt: overwriting ${CHAOS_MOTD_PATH} on ${CHAOS_NODE}"

# ── Apply drift: corrupt MOTD ───────────────────────────────────────────────
# Back up the original MOTD
chaos_backup_file "$CHAOS_MOTD_PATH" "pre_motd_corruption"

# Overwrite with garbage content
cat > "$CHAOS_MOTD_PATH" << 'CHAOS_MOTD'
==============================================
!!! CHAOS INJECTED !!!
This system is in an UNKNOWN state.
DO NOT TRUST THIS MESSAGE.
All services MAY be offline.
Contact the on-call engineer immediately.
==============================================
WARNING: /etc/motd has been intentionally corrupted by the Spindle chaos engine.
The Cinc Client will repair this on the next converge cycle.
CHAOS_MOTD

chmod 0644 "$CHAOS_MOTD_PATH"
chown root:root "$CHAOS_MOTD_PATH"

chaos_log "DRIFT" "MOTD overwritten at $CHAOS_MOTD_PATH"

# Track original content for emergency revert
ORIG_MOTD_CONTENT=$(cat "${CHAOS_BACKUP_DIR}/$(basename "$CHAOS_MOTD_PATH").bak_${CHAOS_TIMESTAMP}" 2>/dev/null || echo "This node is managed by CINC.")
chaos_track_command "restore_motd" "cp '${CHAOS_BACKUP_DIR}/$(basename "$CHAOS_MOTD_PATH").bak_${CHAOS_TIMESTAMP}' '$CHAOS_MOTD_PATH' && chmod 0644 '$CHAOS_MOTD_PATH' && chown root:root '$CHAOS_MOTD_PATH'"

# ── Post-check ──────────────────────────────────────────────────────────────
if ! chaos_finalize; then
    chaos_log "FATAL" "motd-corrupt: safety guard tripped — auto-reverted"
    exit 1
fi

exit 0

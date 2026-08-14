#!/bin/bash
# chaos-user-removal.sh — Drift type 2: user-removal
# Deletes the managed deploy user → fails user-1.0
#
# Fails: user-1.0 (base InSpec profile)
# Repair: cinc-client --once (recipe[base] recreates user)
#
# Usage: chaos-user-removal.sh <target_node> <app>

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../library/chaos_safety.sh"

TARGET_NODE="${1:-}"
TARGET_APP="${2:-}"

if [ -z "$TARGET_NODE" ] || [ -z "$TARGET_APP" ]; then
    echo "Usage: $0 <target_node> <app>"
    exit 1
fi

chaos_init "user-removal" "$TARGET_APP" "$TARGET_NODE" || {
    chaos_log "FATAL" "chaos-user-removal: pre-flight checks failed"
    exit 1
}

chaos_log "APPLY" "user-removal: deleting deploy user on ${CHAOS_NODE}"

# ── Apply drift: remove the deploy user ─────────────────────────────────────
# Back up /etc/passwd and /etc/shadow first
chaos_backup_file "/etc/passwd" "CHANGE_ME"
chaos_backup_file "/etc/shadow" "pre_user_removal_shadow"
chaos_backup_file "/etc/group"  "pre_user_removal_group"

if id "$CHAOS_DEPLOY_USER" >/dev/null 2>&1; then
    chaos_log "DRIFT" "Deleting user $CHAOS_DEPLOY_USER"
    userdel -r "$CHAOS_DEPLOY_USER" 2>/dev/null || userdel "$CHAOS_DEPLOY_USER" 2>/dev/null || true
else
    chaos_log "INFO" "User $CHAOS_DEPLOY_USER already absent — creating then deleting to simulate drift"
    useradd -m -s /bin/bash "$CHAOS_DEPLOY_USER" 2>/dev/null || true
    sleep 1
    userdel -r "$CHAOS_DEPLOY_USER" 2>/dev/null || userdel "$CHAOS_DEPLOY_USER" 2>/dev/null || true
fi

# Track re-creation for manifest / emergency revert
chaos_track_command "recreate_deploy_user" "useradd -m -s /bin/bash ${CHAOS_DEPLOY_USER}"

# ── Post-check ──────────────────────────────────────────────────────────────
if ! chaos_finalize; then
    chaos_log "FATAL" "user-removal: safety guard tripped — auto-reverted"
    exit 1
fi

exit 0

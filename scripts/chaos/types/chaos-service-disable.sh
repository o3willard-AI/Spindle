#!/bin/bash
# chaos-service-disable.sh — Drift type 5: service-disable
# Disables (but does not stop) the node's app service → fails fleet-services enabled
#
# Fails: fleet-services enabled (role Cinc Auditor control)
# Repair: cinc-client --once (service[...] action [:enable, :start])
#
# Usage: chaos-service-disable.sh <target_node> <app>

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../library/chaos_safety.sh"

TARGET_NODE="${1:-}"
TARGET_APP="${2:-}"

if [ -z "$TARGET_NODE" ] || [ -z "$TARGET_APP" ]; then
    echo "Usage: $0 <target_node> <app>"
    exit 1
fi

chaos_init "service-disable" "$TARGET_APP" "$TARGET_NODE" || {
    chaos_log "FATAL" "service-disable: pre-flight checks failed"
    exit 1
}

chaos_log "APPLY" "service-disable: disabling ${CHAOS_SERVICE} on ${CHAOS_NODE}"

# ── Apply drift: disable the app service (leave running if it is) ────────────
# Record state before
chaos_log "INFO" "Pre-disable state: enabled=$(systemctl is-enabled "$CHAOS_SERVICE" 2>/dev/null || echo unknown), active=$(systemctl is-active "$CHAOS_SERVICE" 2>/dev/null || echo unknown)"

# Disable but do NOT stop — the service keeps running, just won't start on boot
systemctl disable "$CHAOS_SERVICE" 2>/dev/null || chaos_log "WARN" "Service $CHAOS_SERVICE disable failed"

chaos_log "DRIFT" "Service $CHAOS_SERVICE disabled on ${CHAOS_NODE}"

# Track re-enable for emergency revert
chaos_track_command "re-enable_service" "systemctl enable ${CHAOS_SERVICE}"

# ── Post-check ──────────────────────────────────────────────────────────────
if ! chaos_finalize; then
    chaos_log "FATAL" "service-disable: safety guard tripped — auto-reverted"
    exit 1
fi

exit 0

#!/bin/bash
# chaos-service-stop.sh — Drift type 4: service-stop
# Stops the node's app service (but keeps it enabled) → fails fleet-services running
#
# Fails: fleet-services running (role Cinc Auditor control)
# Repair: cinc-client --once (service[...] action [:enable, :start])
#
# Usage: chaos-service-stop.sh <target_node> <app>

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../library/chaos_safety.sh"

TARGET_NODE="${1:-}"
TARGET_APP="${2:-}"

if [ -z "$TARGET_NODE" ] || [ -z "$TARGET_APP" ]; then
    echo "Usage: $0 <target_node> <app>"
    exit 1
fi

chaos_init "service-stop" "$TARGET_APP" "$TARGET_NODE" || {
    chaos_log "FATAL" "service-stop: pre-flight checks failed"
    exit 1
}

chaos_log "APPLY" "service-stop: stopping ${CHAOS_SERVICE} on ${CHAOS_NODE}"

# ── Apply drift: stop the app service (leave enabled) ────────────────────────
# Record state before
chaos_log "INFO" "Pre-stop state: enabled=$(systemctl is-enabled "$CHAOS_SERVICE" 2>/dev/null || echo unknown), active=$(systemctl is-active "$CHAOS_SERVICE" 2>/dev/null || echo unknown)"

# Stop the service but DON'T disable it — reconvergence will start it
systemctl stop "$CHAOS_SERVICE" 2>/dev/null || chaos_log "WARN" "Service $CHAOS_SERVICE stop failed (may not be running)"

chaos_log "DRIFT" "Service $CHAOS_SERVICE stopped on ${CHAOS_NODE}"

# Track restart for emergency revert
chaos_track_command "restart_service" "systemctl start ${CHAOS_SERVICE} && systemctl enable ${CHAOS_SERVICE}"

# ── Post-check ──────────────────────────────────────────────────────────────
if ! chaos_finalize; then
    chaos_log "FATAL" "service-stop: safety guard tripped — auto-reverted"
    exit 1
fi

exit 0

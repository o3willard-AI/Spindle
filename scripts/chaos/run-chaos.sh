#!/bin/bash
# run-chaos.sh — Spindle Chaos Engine Orchestrator
#
# Dispatches chaos types to the correct handler. Each chaos type is a
# self-contained script in types/ that sources library/chaos_safety.sh.
#
# Usage:
#   run-chaos.sh <chaos_type> <target_node> <app>
#   run-chaos.sh <chaos_type> <app>            # auto-detect node from app
#   run-chaos.sh --list-types                  # list available chaos types
#   run-chaos.sh --list-nodes                  # list fleet node map
#   run-chaos.sh --dry-run <chaos_type> <app>  # show what would happen
#
# Example:
#   run-chaos.sh service-stop 203.0.113.11 web
#   run-chaos.sh package-purge fleet-01 web
#   run-chaos.sh port-shift web               # auto-resolves to fleet-01 (.211)
#
# Exit codes:
#   0  — chaos applied successfully, safety guards passed
#   1  — pre-flight safety check failed (SSH/Cinc down)
#   2  — invalid arguments
#   3  — safety guard tripped post-apply, auto-reverted

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/library/chaos_safety.sh"

# ── Command dispatch ────────────────────────────────────────────────────────
usage() {
    cat << 'HELP'
Spindle Chaos Engine — Orchestrator
Usage:
  run-chaos.sh <chaos_type> <target_node|app> <app>
  run-chaos.sh --list-types
  run-chaos.sh --list-nodes
  run-chaos.sh --dry-run <chaos_type> <app>

Chaos types:
  1. package-purge     Remove htop/vim/tmux/curl          → fails packages-1.0
  2. user-removal      Delete deploy user                 → fails user-1.0
  3. motd-corrupt      Overwrite /etc/motd                → fails motd-1.0
  4. service-stop      Stop app service                   → fails fleet-services running
  5. service-disable   Disable app service                → fails fleet-services enabled
  6. port-shift        Rewrite listen port in config      → fails http(...) check
  7. config-corrupt    Inject bad directive / truncate    → fails fleet-services + misconfig
  8. permission-drift  chmod/chown managed file           → fails file/perm control

Compliance chaos: types 1-4  |  Misconfiguration chaos: types 5-8
HELP
    exit 2
}

list_types() {
    echo "=== Available Chaos Types ==="
    echo "1. package-purge     (compliance) — packages-1.0"
    echo "2. user-removal      (compliance) — user-1.0"
    echo "3. motd-corrupt      (compliance) — motd-1.0"
    echo "4. service-stop      (compliance) — fleet-services running"
    echo "5. service-disable   (misconfig)  — fleet-services enabled"
    echo "6. port-shift        (misconfig)  — http(...) check"
    echo "7. config-corrupt    (misconfig)  — fleet-services + misconfig"
    echo "8. permission-drift   (misconfig)  — file/perm control"
    echo ""
    echo "Compliance chaos: types 1-4  (detected by base + role InSpec profiles)"
    echo "Misconfiguration chaos: types 5-8 (detected by role InSpec profiles)"
}

list_nodes() {
    echo "=== Fleet Node Map ==="
    printf "%-18s %-14s %-12s %-12s %s\n" "IP" "Node" "Role" "Service" "Config"
    echo "─────────────────────────────────────────────────────────────────────────"
    for entry in "${CHAOS_FLEET_NODES[@]}"; do
        local ip role node svc cfg
        ip=$(echo "$entry" | cut -d'|' -f1)
        role=$(echo "$entry" | cut -d'|' -f2)
        node=$(echo "$entry" | cut -d'|' -f3)
        svc=$(echo "$entry" | cut -d'|' -f4)
        cfg=$(echo "$entry" | cut -d'|' -f5)
        printf "%-18s %-14s %-12s %-12s %s\n" "$ip" "$node" "$role" "$svc" "$cfg"
    done
}

# Resolve a node identifier (IP or hostname) to a fleet entry
resolve_node() {
    local target="$1"
    for entry in "${CHAOS_FLEET_NODES[@]}"; do
        local ip role node svc cfg
        ip=$(echo "$entry" | cut -d'|' -f1)
        role=$(echo "$entry" | cut -d'|' -f2)
        node=$(echo "$entry" | cut -d'|' -f3)
        svc=$(echo "$entry" | cut -d'|' -f4)
        cfg=$(echo "$entry" | cut -d'|' -f5)

        if [ "$ip" = "$target" ] || [ "$node" = "$target" ]; then
            NODE_IP="$ip"
            NODE_ROLE="$role"
            NODE_NAME="$node"
            NODE_SERVICE="$svc"
            NODE_CONFIG="$cfg"
            return 0
        fi
    done
    return 1
}

# ── Argument parsing ────────────────────────────────────────────────────────
if [ "${1:-}" = "--list-types" ]; then list_types; exit 0; fi
if [ "${1:-}" = "--list-nodes" ]; then list_nodes; exit 0; fi

# Dry-run mode: show what would happen without executing
if [ "${1:-}" = "--dry-run" ]; then
    shift
    CHAOS_TYPE="${1:-}"
    TARGET_APP="${2:-}"
    if [ -z "$CHAOS_TYPE" ] || [ -z "$TARGET_APP" ]; then
        echo "Dry-run requires: --dry-run <chaos_type> <app>"
        exit 2
    fi
    echo "=== DRY RUN ==="
    echo "Type:  $CHAOS_TYPE"
    echo "App:   $TARGET_APP"
    echo "Target node: auto-resolve from app map"
    echo "(No changes will be made. Remove --dry-run to execute.)"
    exit 0
fi

CHAOS_TYPE="${1:-}"
TARGET_ARG="${2:-}"
TARGET_APP="${3:-}"

if [ -z "$CHAOS_TYPE" ] || [ -z "$TARGET_ARG" ] || [ -z "$TARGET_APP" ]; then
    usage
fi

# Validate chaos type
VALID_TYPES="package-purge user-removal motd-corrupt service-stop service-disable port-shift config-corrupt permission-drift"
if ! echo "$VALID_TYPES" | grep -qw "$CHAOS_TYPE"; then
    echo "ERROR: Unknown chaos type '$CHAOS_TYPE'"
    echo "Valid types: package-purge, user-removal, motd-corrupt, service-stop,"
    echo "             service-disable, port-shift, config-corrupt, permission-drift"
    exit 2
fi

# Map chaos type to script
CHAOS_SCRIPT="${SCRIPT_DIR}/types/chaos-${CHAOS_TYPE}.sh"

if [ ! -f "$CHAOS_SCRIPT" ]; then
    echo "ERROR: Script $CHAOS_SCRIPT not found"
    exit 2
fi

# Resolve target: TARGET_ARG can be an IP, a hostname (fleet-01), or if it
# matches the app, treat it as the app and auto-resolve the node.
# Pattern: <chaos_type> <target_node> <app>  OR  <chaos_type> <app>
if [ -n "${4:-}" ]; then
    # Full form: chaos_type node app
    TARGET_NODE="$TARGET_ARG"
    TARGET_APP="$TARGET_APP"
else
    # Abbreviated form: chaos_type app — auto-resolve node from app
    TARGET_NODE=""
    TARGET_APP="$TARGET_ARG"
fi

# Execute the chaos script
chaos_log "ORCHESTRATOR" "Dispatching: type=$CHAOS_TYPE target=${TARGET_NODE:-auto} app=$TARGET_APP"

chmod +x "$CHAOS_SCRIPT"
bash "$CHAOS_SCRIPT" "$TARGET_NODE" "$TARGET_APP"
EXIT_CODE=$?

case $EXIT_CODE in
    0) chaos_log "ORCHESTRATOR" "Chaos $CHAOS_TYPE completed successfully" ;;
    1) chaos_log "ORCHESTRATOR" "Chaos $CHAOS_TYPE aborted (safety pre-flight)" ;;
    3) chaos_log "ORCHESTRATOR" "Chaos $CHAOS_TYPE auto-reverted (safety post-flight)" ;;
    *) chaos_log "ORCHESTRATOR" "Chaos $CHAOS_TYPE exited with code $EXIT_CODE" ;;
esac

exit $EXIT_CODE

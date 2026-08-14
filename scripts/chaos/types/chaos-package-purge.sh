#!/bin/bash
# chaos-package-purge.sh — Drift type 1: package-purge
# Removes managed base packages (htop, vim, tmux, curl) → fails packages-1.0
#
# Fails: packages-1.0 (base InSpec profile)
# Repair: cinc-client --once (recipe[base] reinstalls packages)
#
# Usage: chaos-package-purge.sh <target_node> <app>

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/library/chaos_safety.sh"

TARGET_NODE="${1:-}"
TARGET_APP="${2:-}"

if [ -z "$TARGET_NODE" ] || [ -z "$TARGET_APP" ]; then
    echo "Usage: $0 <target_node> <app>"
    echo "  target_node: IP or hostname (e.g. 198.51.100.211 or fleet-01)"
    echo "  app: app identifier (e.g. web, database, loadbalancer)"
    exit 1
fi

# Initialize chaos context (runs pre-flight safety checks)
chaos_init "package-purge" "$TARGET_APP" "$TARGET_NODE" || {
    chaos_log "FATAL" "chaos-package-purge: pre-flight checks failed"
    exit 1
}

chaos_log "APPLY" "package-purge: removing base packages ($CHAOS_BASE_PACKAGES) on ${CHAOS_NODE}"

# ── Apply drift: purge the packages ─────────────────────────────────────────
for pkg in $CHAOS_BASE_PACKAGES; do
    if dpkg -l "$pkg" >/dev/null 2>&1; then
        chaos_log "DRIFT" "Purging $pkg"
        apt-get purge -y "$pkg" >/dev/null 2>&1 || true
    else
        chaos_log "INFO" "Package $pkg already absent — nothing to purge"
    fi
done

# Clean up apt caches to ensure packages don't linger
apt-get autoremove -y >/dev/null 2>&1 || true

# Track the re-install command for manifest / emergency revert
chaos_track_command "reinstall_purged_packages" "apt-get install -y $CHAOS_BASE_PACKAGES"

# ── Post-check: verify SSH + Cinc still alive ───────────────────────────────
if ! chaos_finalize; then
    chaos_log "FATAL" "package-purge: safety guard tripped — auto-reverted"
    exit 1
fi

exit 0

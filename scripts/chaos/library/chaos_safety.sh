#!/bin/bash
# chaos_safety.sh — Shared safety library for the Spindle chaos engine.
#
# Provides:
#   - Pre-flight guardrails (SSH + Cinc alive, no destructive targets)
#   - Post-flight verification of SSH + Cinc liveness
#   - Backup-before-mutate helpers
#   - Auto-revert on guard trip with structured manifest output
#
# Usage:
#   source /opt/spindle/scripts/chaos/library/chaos_safety.sh
#   chaos_init "package-purge" "$TARGET_APP"
#   ... apply drift ...
#   chaos_finalize "$TARGET_APP"
#

# ── Fleet node map ──────────────────────────────────────────────────────────
# Each entry: IP|role|app_service|config_file|role_inspec_profile
CHAOS_FLEET_NODES=(
    "203.0.113.11|web|fleet-01|apache2|/etc/apache2/ports.conf"
    "203.0.113.12|database|fleet-02|postgresql|/etc/postgresql/16/main/conf.d/spindle-tuning.conf"
    "203.0.113.13|loadbalancer|fleet-03|haproxy|/etc/haproxy/haproxy.cfg"
)

# Packages managed by the base cookbook (for package-purge chaos)
CHAOS_BASE_PACKAGES="htop vim tmux curl"

# Deploy user managed by base cookbook (for user-removal chaos)
CHAOS_DEPLOY_USER="deploy"

# MOTD path managed by base cookbook
CHAOS_MOTD_PATH="/etc/motd"

# ── Globals initialized by chaos_init ───────────────────────────────────────
CHAOS_TYPE=""
CHAOS_APP=""
CHAOS_NODE=""
CHAOS_IP=""
CHAOS_ROLE=""
CHAOS_SERVICE=""
CHAOS_CONFIG=""
CHAOS_PROFILE=""
CHAOS_TIMESTAMP=""
CHAOS_MANIFEST=""
CHAOS_BACKUP_DIR=""
CHAOS_CHANGED_FILES=()
CHAOS_CHANGED_COMMANDS=()
CHAOS_SAFE_MODE=true

# ── Logging ─────────────────────────────────────────────────────────────────
# Use /var/log when root, fall back to $HOME/.chaos/logs otherwise
if [ -w "/var/log" ] 2>/dev/null; then
    CHAOS_LOG="/var/log/chaos/chaos-engine.log"
    CHAOS_DEFAULT_BACKUP_DIR="/var/backups"
else
    CHAOS_LOG="${HOME}/.chaos/logs/chaos/chaos-engine.log"
    CHAOS_DEFAULT_BACKUP_DIR="${HOME}/.chaos/backups/chaos"
fi
mkdir -p "$(dirname "$CHAOS_LOG")" 2>/dev/null || true

chaos_log() {
    local level="$1"
    shift
    local ts
    ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    local msg="[$ts] [$level] $*"
    echo "$msg" >> "$CHAOS_LOG" 2>/dev/null || true
    echo "$msg"
}

# ── Emergency revert: restore all backed-up files and commands ────────────
chaos_emergency_revert() {
    chaos_log "FATAL" "Safety guard tripped — initiating emergency revert"
    local restored=0

    # Restore backed-up files
    for entry in "${CHAOS_CHANGED_FILES[@]}"; do
        # entry format: current_file|backup_file|description
        local current="${entry%%|*}"
        local rest="${entry#*|}"
        local backup="${rest%%|*}"
        local desc="${rest#*|}"

        if [ -f "$backup" ]; then
            cp "$backup" "$current" 2>/dev/null && {
                chaos_log "REVERT" "Restored $current ($desc) from $backup"
                restored=$((restored + 1))
            } || chaos_log "ERROR" "Failed to restore $current from $backup"
        fi
    done

    # Re-run restore commands if any
    for cmd in "${CHAOS_CHANGED_COMMANDS[@]}"; do
        local desc="${cmd%%|*}"
        local restore_cmd="${cmd#*|}"
        chaos_log "REVERT" "Executing: $restore_cmd ($desc)"
        eval "$restore_cmd" 2>/dev/null || chaos_log "ERROR" "Restore command failed: $restore_cmd"
        restored=$((restored + 1))
    done

    # Restart services that we may have stopped/disabled
    if [ -n "${CHAOS_SERVICE:-}" ]; then
        systemctl start "$CHAOS_SERVICE" 2>/dev/null || true
        systemctl enable "$CHAOS_SERVICE" 2>/dev/null || true
    fi

    # Reinstall purged packages
    if [ "${CHAOS_TYPE}" == "package-purge" ]; then
        for pkg in $CHAOS_BASE_PACKAGES; do
            apt-get install -y "$pkg" >/dev/null 2>&1 || true
        done
    fi

    # Recreate removed user
    if [ "${CHAOS_TYPE}" == "user-removal" ]; then
        id "$CHAOS_DEPLOY_USER" 2>/dev/null || useradd -m -s /bin/bash "$CHAOS_DEPLOY_USER" 2>/dev/null || true
    fi

    chaos_log "REVERT" "Emergency revert complete — restored $restored item(s)"
    return 0
}

# ── Pre-flight: verify SSH + Cinc are alive BEFORE any damage ───────────────
chaos_assert_safe_to_proceed() {
    # SSH must be alive
    if ! systemctl is-active ssh.service >/dev/null 2>&1; then
        chaos_log "FATAL" "SSH is not active — ABORTING for safety"
        return 1
    fi

    # Cinc Client must be installed (check both service and binary)
    local cinc_alive=false
    if systemctl is-active cinc-client.service >/dev/null 2>&1; then
        cinc_alive=true
    elif systemctl is-active cinc.service >/dev/null 2>&1; then
        cinc_alive=true
    elif command -v cinc-client >/dev/null 2>&1; then
        cinc_alive=true
    fi

    if [ "$cinc_alive" = false ]; then
        chaos_log "FATAL" "Cinc Client is not alive — ABORTING to preserve recovery path"
        return 1
    fi

    # We must not be running as a service we are about to kill
    if systemctl is-active cinc-client.service >/dev/null 2>&1; then
        CHAOS_CINC_WAS_ACTIVE=true
    fi

    chaos_log "OK" "Pre-flight passed: SSH active, Cinc Client alive"
    return 0
}

# ── Post-flight: verify SSH + Cinc still alive AFTER damage ────────────────
chaos_assert_still_alive() {
    local errors=0

    # SSH check
    if ! systemctl is-active ssh.service >/dev/null 2>&1; then
        chaos_log "FATAL" "POST-CHECK FAILED: SSH service is down after chaos"
        errors=$((errors + 1))
    fi

    # Cinc check
    local cinc_ok=false
    if systemctl is-active cinc-client.service >/dev/null 2>&1; then
        cinc_ok=true
    elif systemctl is-active cinc.service >/dev/null 2>&1; then
        cinc_ok=true
    elif command -v cinc-client >/dev/null 2>&1; then
        cinc_ok=true
    fi

    if [ "$cinc_ok" = false ]; then
        chaos_log "FATAL" "POST-CHECK FAILED: Cinc Client is no longer alive after chaos"
        errors=$((errors + 1))
    fi

    if [ "$errors" -gt 0 ]; then
        chaos_log "FATAL" "Safety guard tripped — reverting all changes"
        chaos_emergency_revert
        # Re-verify after revert
        if ! systemctl is-active ssh.service >/dev/null 2>&1; then
            chaos_log "FATAL" "SSH still down after revert — MANUAL INTERVENTION REQUIRED"
        fi
        if ! command -v cinc-client >/dev/null 2>&1 && ! systemctl is-active cinc-client.service >/dev/null 2>&1; then
            chaos_log "FATAL" "Cinc Client still missing after revert — MANUAL INTERVENTION REQUIRED"
        fi
        return 1
    fi

    chaos_log "OK" "Post-check passed: SSH + Cinc still alive"
    return 0
}

# ── Backup helper: copy a file before modifying it ─────────────────────────
# Usage: chaos_backup_file /path/to/file description
chaos_backup_file() {
    local file="$1"
    local desc="$2"

    if [ -f "$file" ]; then
        local backup="${CHAOS_BACKUP_DIR}/$(basename "$file").bak_${CHAOS_TIMESTAMP}"
        cp "$file" "$backup"
        CHAOS_CHANGED_FILES+=("${file}|${backup}|${desc}")
        chaos_log "BACKUP" "Backed up $file → $backup ($desc)"
    else
        chaos_log "WARN" "File does not exist for backup: $file ($desc)"
    fi
}

# ── Track a change command for revert ──────────────────────────────────────
# Usage: chaos_track_command "description" "restore command"
chaos_track_command() {
    local desc="$1"
    local cmd="$2"
    CHAOS_CHANGED_COMMANDS+=("${desc}|${cmd}")
}

# ── Write manifest for InSpec/Cinc recovery ───────────────────────────────
chaos_write_manifest() {
    mkdir -p "$(dirname "$CHAOS_MANIFEST")"
    {
        echo "node:${CHAOS_NODE}"
        echo "role:${CHAOS_ROLE}"
        echo "ip:${CHAOS_IP}"
        echo "app:${CHAOS_APP}"
        echo "chaos_type:${CHAOS_TYPE}"
        echo "timestamp:${CHAOS_TIMESTAMP}"
        echo "backup_dir:${CHAOS_BACKUP_DIR}"
        echo ""
        echo "changed_files:"
        for entry in "${CHAOS_CHANGED_FILES[@]}"; do
            local current="${entry%%|*}"
            local rest="${entry#*|}"
            local backup="${rest%%|*}"
            local desc="${rest#*|}"
            echo "  - file:${current}"
            echo "    action:${desc}"
            echo "    backup:${backup}"
        done
        echo ""
        echo "changed_commands:"
        for entry in "${CHAOS_CHANGED_COMMANDS[@]}"; do
            local desc="${entry%%|*}"
            local cmd="${entry#*|}"
            echo "  - desc:${desc}"
            echo "    command:${cmd}"
        done
        echo ""
        echo "restore:"
        echo "  method:${CHAOS_TYPE}"
        echo "  cinc_repair_recipe:recipe[spindle-qa::${CHAOS_ROLE}]"
        echo "  cinc_repair_base:recipe[base]"
    } > "$CHAOS_MANIFEST"
    chaos_log "MANIFEST" "Written to $CHAOS_MANIFEST"
    echo "$CHAOS_MANIFEST"
}

# ── Initialize chaos context from node + app ──────────────────────────────
# Usage: chaos_init <chaos_type> <app> <node_ip>
chaos_init() {
    CHAOS_TYPE="$1"
    CHAOS_APP="$2"
    local target_ip="${3:-}"

    CHAOS_TIMESTAMP=$(date +%Y%m%d_%H%M%S)
    CHAOS_BACKUP_DIR="${CHAOS_DEFAULT_BACKUP_DIR}/chaos_${CHAOS_TYPE}_${CHAOS_TIMESTAMP}"
    mkdir -p "$CHAOS_BACKUP_DIR" 2>/dev/null || {
        CHAOS_BACKUP_DIR="/tmp/chaos_${CHAOS_TYPE}_${CHAOS_TIMESTAMP}"
        mkdir -p "$CHAOS_BACKUP_DIR"
    }

    # Resolve node from fleet map (by IP or node name like "fleet-01")
    CHAOS_NODE=""
    CHAOS_ROLE=""
    CHAOS_SERVICE=""
    CHAOS_CONFIG=""
    CHAOS_PROFILE=""

    for entry in "${CHAOS_FLEET_NODES[@]}"; do
        local ip role node svc cfg
        ip=$(echo "$entry" | cut -d'|' -f1)
        role=$(echo "$entry" | cut -d'|' -f2)
        node=$(echo "$entry" | cut -d'|' -f3)
        svc=$(echo "$entry" | cut -d'|' -f4)
        cfg=$(echo "$entry" | cut -d'|' -f5)

        if [ -n "$target_ip" ] && { [ "$ip" = "$target_ip" ] || [ "$node" = "$target_ip" ]; }; then
            CHAOS_IP="$ip"
            CHAOS_NODE="$node"
            CHAOS_ROLE="$role"
            CHAOS_SERVICE="$svc"
            CHAOS_CONFIG="$cfg"
            break
        fi
    done

    # If no IP specified, try to detect from hostname
    if [ -z "$CHAOS_IP" ]; then
        local hname
        hname=$(hostname)
        for entry in "${CHAOS_FLEET_NODES[@]}"; do
            local ip role node svc cfg
            ip=$(echo "$entry" | cut -d'|' -f1)
            role=$(echo "$entry" | cut -d'|' -f2)
            node=$(echo "$entry" | cut -d'|' -f3)
            svc=$(echo "$entry" | cut -d'|' -f4)
            cfg=$(echo "$entry" | cut -d'|' -f5)

            # Match by node name suffix: fleet-01 → 203.0.113.11 etc.
            local suffix
            suffix=$(echo "$hname" | sed 's/fleet-0/0/')
            if [ "$node" = "$hname" ] || [ "$node" = "$suffix" ]; then
                CHAOS_IP="$ip"
                CHAOS_NODE="$node"
                CHAOS_ROLE="$role"
                CHAOS_SERVICE="$svc"
                CHAOS_CONFIG="$cfg"
                break
            fi
        done
    fi

    # Resolve app → map to service/config/profile
    # The $CHAOS_APP may be a high-level app name; map it to node service.
    if [ -z "$CHAOS_IP" ]; then
        case "$CHAOS_APP" in
            web|apache|nginx|enterprise-portal|spindle-web)
                CHAOS_IP="203.0.113.11"
                CHAOS_NODE="fleet-01"
                CHAOS_ROLE="web"
                CHAOS_SERVICE="apache2"
                CHAOS_CONFIG="/etc/apache2/ports.conf"
                CHAOS_PROFILE="web"
                ;;
            database|postgres|postgresql|spindle-db)
                CHAOS_IP="203.0.113.12"
                CHAOS_NODE="fleet-02"
                CHAOS_ROLE="database"
                CHAOS_SERVICE="postgresql"
                CHAOS_CONFIG="/etc/postgresql/16/main/conf.d/spindle-tuning.conf"
                CHAOS_PROFILE="database"
                ;;
            loadbalancer|haproxy|lb|spindle-lb)
                CHAOS_IP="203.0.113.13"
                CHAOS_NODE="fleet-03"
                CHAOS_ROLE="loadbalancer"
                CHAOS_SERVICE="haproxy"
                CHAOS_CONFIG="/etc/haproxy/haproxy.cfg"
                CHAOS_PROFILE="loadbalancer"
                ;;
            *)
                chaos_log "FATAL" "Unknown app: $CHAOS_APP"
                return 1
                ;;
        esac
    fi

    # If no IP matched at all, fail
    if [ -z "$CHAOS_IP" ]; then
        chaos_log "FATAL" "Cannot resolve node for app=$CHAOS_APP ip=$target_ip"
        return 1
    fi

    # Determine InSpec profile path
    case "$CHAOS_ROLE" in
        web)        CHAOS_PROFILE="web" ;;
        database)   CHAOS_PROFILE="database" ;;
        loadbalancer) CHAOS_PROFILE="loadbalancer" ;;
    esac

    # The chaos-manifest path
    CHAOS_MANIFEST="${CHAOS_BACKUP_DIR}/chaos-manifest.yaml"

    chaos_log "INIT" "chaos_init: type=$CHAOS_TYPE app=$CHAOS_APP node=$CHAOS_NODE($CHAOS_IP) role=$CHAOS_ROLE service=$CHAOS_SERVICE config=$CHAOS_CONFIG profile=$CHAOS_PROFILE"

    # Run the safety pre-flight
    chaos_assert_safe_to_proceed || return 1
    return 0
}

# ── Finalize: post-check + write manifest ───────────────────────────────────
chaos_finalize() {
    # Post-flight safety check
    if ! chaos_assert_still_alive; then
        chaos_log "FATAL" "chaos_finalize: safety guard tripped during finalization"
        return 1
    fi

    # Write the manifest
    local manifest
    manifest=$(chaos_write_manifest)
    chaos_log "DONE" "Chaos $CHAOS_TYPE applied on ${CHAOS_NODE}. Manifest: $manifest"

    echo ""
    echo "=== Chaos Applied Successfully ==="
    echo "  Type:     $CHAOS_TYPE"
    echo "  Node:     $CHAOS_NODE ($CHAOS_IP)"
    echo "  Role:     $CHAOS_ROLE"
    echo "  Service:  $CHAOS_SERVICE"
    echo "  Timestamp: $CHAOS_TIMESTAMP"
    echo "  Manifest: $manifest"
    echo "  Backups:  $CHAOS_BACKUP_DIR"
    echo ""
    echo "  To auto-repair: cinc-client --once (will converge $CHAOS_ROLE recipe)"
    echo "  To manual-revert: cp $CHAOS_BACKUP_DIR/* <restored paths>"
    echo "=================================="
    return 0
}

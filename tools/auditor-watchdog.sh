#!/bin/bash
# auditor-watchdog.sh — Cinc Auditor → Cinc bridge
#
# Runs the node's Cinc Auditor compliance profile(s), and if any control fails (i.e.
# a deviation is detected), triggers a Cinc Client converge to repair it, then
# re-scans to confirm the node is clean.
#
# Intended as the Exec step of the Cinc Auditor timer (spindle-auditor-scan.service),
# replacing a bare "cinc-auditor exec" with a conditional converge-trigger.
#
# Usage: auditor-watchdog.sh [profile-dir]   (default: detect role profile)
# Env:   AUDITOR_BIN, AUDITOR_TIMEOUT, CONVERGE_SCRIPT, PROFILE_ROOT
#        STATUS_PARSER (path to auditor_json_status.py)

set -uo pipefail

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
NODE=$(hostname)
AUDITOR_BIN="${AUDITOR_BIN:-/usr/bin/cinc-auditor}"
CONVERGE_SCRIPT="${CONVERGE_SCRIPT:-/opt/spindle/scripts/cinc/run-converge.sh}"
PROFILE_ROOT="${PROFILE_ROOT:-/tmp/spindle-qa/auditor}"
AUDITOR_TIMEOUT="${AUDITOR_TIMEOUT:-120}"
# Locate the status parser relative to this script or in PATH
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -f "$SCRIPT_DIR/auditor_json_status.py" ]; then
    STATUS_PARSER="$SCRIPT_DIR/auditor_json_status.py"
else
    STATUS_PARSER="${STATUS_PARSER:-/opt/spindle/scripts/auditor-scan/auditor_json_status.py}"
fi

LOGDIR=/var/log/spindle/auditor-watchdog
mkdir -p "$LOGDIR"
REPORT="$LOGDIR/${NODE}-${TIMESTAMP}.json"
LOG="$LOGDIR/${NODE}-${TIMESTAMP}.log"

log() { echo "[$(date +%Y%m%d_%H%M%S)] $*" >> "$LOG"; }
log "=== Cinc Auditor watchdog start on $NODE ==="

# --- Detect role profile -------------------------------------------------
if [ -n "${1:-}" ] && [ -d "$1" ]; then
    PROFILE_DIR="$1"
    log "Using explicit profile: $PROFILE_DIR"
else
    ROLE=""
    if [ -f "/var/chef/nodes/${NODE}.json" ]; then
        ROLE=$(python3 -c '
import json,re,sys
d=json.load(open(sys.argv[1]))
rl=d.get("run_list",[])
recipe=rl[0] if rl else ""
m=re.search(r"::([a-z_0-9]+)", recipe)
print(m.group(1) if m else "")
' "/var/chef/nodes/${NODE}.json" 2>/dev/null || true)
    fi
    [ -z "$ROLE" ] && ROLE="web_app"
    BASE=$(echo "$ROLE" | sed 's/_app$//')
    PROFILE_DIR="$PROFILE_ROOT/$BASE"
    if [ ! -d "$PROFILE_DIR" ]; then
        log "Profile '$PROFILE_DIR' missing for role '$ROLE'; falling back to web."
        PROFILE_DIR="$PROFILE_ROOT/web"
    fi
    [ -d "$PROFILE_DIR" ] || { log "ERROR: no profile found under $PROFILE_DIR"; exit 1; }
    log "Detected role '$ROLE' -> profile $PROFILE_DIR"
fi

# --- 1) SCAN --------------------------------------------------------------
log "Running Cinc Auditor profile: $PROFILE_DIR"
timeout "$AUDITOR_TIMEOUT" "$AUDITOR_BIN" exec "$PROFILE_DIR" --reporter json:"$REPORT" >/dev/null 2>&1
if [ ! -s "$REPORT" ]; then
    log "ERROR: no Cinc Auditor report produced. Aborting without converge."
    exit 1
fi

# --- 2) EVALUATE ----------------------------------------------------------
read FAILED_COUNT TOTAL_COUNT SKIPPED_COUNT <<<"$(python3 "$STATUS_PARSER" "$REPORT" 2>/dev/null)"
FAILED_COUNT=${FAILED_COUNT:-0}
log "Scan result: failed=$FAILED_COUNT total=$TOTAL_COUNT skipped=$SKIPPED_COUNT"

if [ "$FAILED_COUNT" -eq 0 ]; then
    log "Node is COMPLIANT — no converge needed."
    echo "[$TIMESTAMP] COMPLIANT:$NODE"
    exit 0
fi

# --- 3) REPAIR ------------------------------------------------------------
log "DEVIATION DETECTED (failed=$FAILED_COUNT) — triggering Cinc converge via $CONVERGE_SCRIPT"
if [ -x "$CONVERGE_SCRIPT" ]; then
    "$CONVERGE_SCRIPT" >>"$LOG" 2>&1
    log "Converge returned $?."
else
    log "Converge script $CONVERGE_SCRIPT not executable — skipped."
fi

# --- 4) RE-SCAN -----------------------------------------------------------
REPORT2="$LOGDIR/${NODE}-${TIMESTAMP}-post.json"
timeout "$AUDITOR_TIMEOUT" "$AUDITOR_BIN" exec "$PROFILE_DIR" --reporter json:"$REPORT2" >/dev/null 2>&1
if [ -s "$REPORT2" ]; then
    read POST_FAILED _ <<<"$(python3 "$STATUS_PARSER" "$REPORT2" 2>/dev/null)"
    if [ "${POST_FAILED:-999}" -eq 0 ] 2>/dev/null; then
        log "REMEDIATED after converge (0 failed)."
        echo "[$TIMESTAMP] REMEDIATED:$NODE"
        exit 0
    else
        log "STILL FAILING after converge (failed=$POST_FAILED)."
        echo "[$TIMESTAMP] STILL_FAILING:$NODE failed=$POST_FAILED"
        exit 2
    fi
else
    log "No post-converge report produced."
    exit 2
fi

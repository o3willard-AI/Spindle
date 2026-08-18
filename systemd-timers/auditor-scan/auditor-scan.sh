#!/bin/bash
# auditor-scan.sh — run Cinc Auditor profiles on THIS node and POST each report
# to Spindle's /ingest/events/auditor (Bearer auth).
#
# Installed on each managed node as a systemd oneshot + timer. Every run scans the
# local node's InSpec profiles and posts the JSON so Spindle records a
# compliance_report + control_results for this node.
#
# Configuration (env vars, or via an EnvironmentFile in the systemd unit):
#   SPINDLE_URL           Spindle server base URL       (default http://127.0.0.1:3000)
#   SPINDLE_INGEST_TOKEN  Bearer token shared with Spindle (REQUIRED)
#   SPINDLE_ORG           organization label stamped on reports (default "default")
#   NODE_NAME             node name to report as        (default: `hostname -s`)
#   PROFILES_DIR          directory holding the InSpec profiles to run
#                         (default /opt/spindle/profiles)
#
# NODE_NAME must match the name Spindle already has for this node (from the CINC
# data-collector path) so compliance and converge data join on the same node.

set -uo pipefail

SPINDLE_URL="${SPINDLE_URL:-http://127.0.0.1:3000}"
INGEST_URL="${SPINDLE_URL%/}/ingest/events/auditor"
TOKEN="${SPINDLE_INGEST_TOKEN:-}"
ORG="${SPINDLE_ORG:-default}"
NODE_NAME="${NODE_NAME:-$(hostname -s)}"
PROFILES_DIR="${PROFILES_DIR:-/opt/spindle/profiles}"
LOGDIR="${LOGDIR:-/var/log/spindle/auditor-scan}"

[ -n "$TOKEN" ] || { echo "ERROR: SPINDLE_INGEST_TOKEN is not set" >&2; exit 1; }
[ -d "$PROFILES_DIR" ] || { echo "ERROR: PROFILES_DIR '$PROFILES_DIR' not found" >&2; exit 1; }
mkdir -p "$LOGDIR"
TS=$(date -u '+%Y-%m-%dT%H:%M:%SZ')

for profile_dir in "$PROFILES_DIR"/*/; do
  [ -f "$profile_dir/inspec.yml" ] || continue
  profile="$(basename "$profile_dir")"
  raw="$LOGDIR/${profile}.json"

  # NOTE: cinc-auditor exits non-zero when a control FAILS while still emitting
  # valid JSON. Do NOT gate posting on the exit code — post whenever the JSON is
  # parseable, so non-compliant (RED) runs are reported to Spindle, not dropped.
  /usr/bin/cinc-auditor exec "$profile_dir" --reporter json --no-distinct-exit \
    >"$raw" 2>"$raw.err" || true

  if ! python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$raw" 2>/dev/null; then
    echo "[$TS] $profile: no valid JSON (see $raw.err)"
    continue
  fi

  # Raw inspec JSON carries platform.name (the OS, e.g. "ubuntu"), not the node
  # hostname. Inject node_name + organization so Spindle keys the report to the
  # correct node (and the worker dedups on that name).
  python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["node_name"]=sys.argv[2]; d["organization"]=sys.argv[3]; json.dump(d, sys.stdout)' \
    "$raw" "$NODE_NAME" "$ORG" >"$raw.post" || { echo "[$TS] $profile: inject failed"; continue; }

  code=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$INGEST_URL" \
    -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
    --data-binary @"$raw.post")
  echo "[$TS] $profile -> HTTP $code"
done
echo "[$TS] auditor scan complete"

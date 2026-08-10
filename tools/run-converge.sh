#!/bin/bash
# Cinc Client Convergence - run-converge.sh
# Runs server-backed converge to repair misconfigurations (fleet-02/03 wiring).
# Uses -c /etc/cinc/client.rb (real Chef server at chef_server_url, cookbook
# fetched from the org); data_collector twin-write ships the converge result to
# Spindle via the proxy.

set -euo pipefail
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOGDIR=/var/log/spindle/cinc-converges
mkdir -p "$LOGDIR"

NODE=$(hostname)
echo "[$TIMESTAMP] Starting cinc converge on $NODE" >> "$LOGDIR/run.log"

# Derive the role recipe from the node run_list if present, else default to web_app
RUNLIST="recipe[spindle-qa::web_app]"
if [ -f "/var/chef/nodes/${NODE}.json" ]; then
    JSON_RUNLIST=$(python3 -c "import json,sys;d=json.load(open(sys.argv[1]));r=d.get('run_list',[]);print(r[0] if r else '')" "/var/chef/nodes/${NODE}.json" 2>/dev/null || true)
    if [ -n "$JSON_RUNLIST" ]; then
        RUNLIST="$JSON_RUNLIST"
    fi
fi

echo "[$TIMESTAMP] Converging runlist: $RUNLIST" >> "$LOGDIR/run.log"

if sudo cinc-client -c /etc/cinc/client.rb --override-runlist "$RUNLIST" 2>&1 | tee "$LOGDIR/converge-${TIMESTAMP}.log"; then
    echo "[$TIMESTAMP] Converge complete on $NODE" >> "$LOGDIR/run.log"
else
    EXIT=$?
    echo "[$TIMESTAMP] Converge exited $EXIT on $NODE" >> "$LOGDIR/run.log"
fi

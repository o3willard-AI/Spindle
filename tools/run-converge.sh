#!/bin/bash
# Cinc Client Convergence - run-converge.sh
# Runs server-backed converge to repair misconfigurations detected by Cinc Auditor.
# Uses local-mode Cinc with cookbook_path pointing to
# /var/cinc/cache/cookbooks + /var/chef/cookbooks.
set -euo pipefail
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOGDIR=/var/log/spindle/cinc-converges
mkdir -p "$LOGDIR"

NODE=$(hostname)
echo "[$TIMESTAMP] Starting cinc converge on $NODE" >> "$LOGDIR/run.log"

# Derive the spindle-qa recipe from hostname
case "$NODE" in
  fleet-01|*211*) RUNLIST="recipe[spindle-qa::web_app]" ;;
  fleet-02|*212*) RUNLIST="recipe[spindle-qa::database]" ;;
  fleet-03|*213*) RUNLIST="recipe[spindle-qa::loadbalancer]" ;;
  *) RUNLIST="recipe[base]" ;;
esac

echo "[$TIMESTAMP] Converging runlist: $RUNLIST" >> "$LOGDIR/run.log"

# Run Cinc in local mode with the override runlist
# -c /etc/cinc/client.rb provides local_mode, cookbook_path, and log settings
sudo cinc-client -c /etc/cinc/client.rb --override-runlist "$RUNLIST" 2>&1 | tee "$LOGDIR/converge-${TIMESTAMP}.log"
RC=$?
echo "[$TIMESTAMP] Converge exited $RC on $NODE" >> "$LOGDIR/run.log"
exit $RC

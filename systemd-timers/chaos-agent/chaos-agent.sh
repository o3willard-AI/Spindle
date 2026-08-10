#!/bin/bash
#!/bin/bash
set -euo pipefail
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOGDIR=/var/log/spindle/chaos-reports
mkdir -p "$LOGDIR"

echo "[$TIMESTAMP] Chaos agent triggered" >> "$LOGDIR/run.log"

# Fleet-01 web_app chaos
if /tmp/chaos-web_app.sh 2>&1 | tee "$LOGDIR/fleet-01-${TIMESTAMP}.log"; then
    echo "[OK] fleet-01 chaos complete"
else
    echo "[WARN] fleet-01 chaos failed (exit $?)"
fi

sleep 5

# Fleet-02 database chaos
if /tmp/chaos-database.sh 2>&1 | tee "$LOGDIR/fleet-02-${TIMESTAMP}.log"; then
    echo "[OK] fleet-02 chaos complete"
else
    echo "[WARN] fleet-02 chaos failed (exit $?)"
fi

sleep 5

# Fleet-03 loadbalancer chaos
if /tmp/chaos-loadbalancer.sh 2>&1 | tee "$LOGDIR/fleet-03-${TIMESTAMP}.log"; then
    echo "[OK] fleet-03 chaos complete"
else
    echo "[WARN] fleet-03 chaos failed (exit $?)"
fi

echo "[$TIMESTAMP] All chaos complete" >> "$LOGDIR/run.log"
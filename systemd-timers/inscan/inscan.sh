#!/bin/bash
#!/bin/bash
set -euo pipefail
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
REPORTS_DIR=/var/log/spindle/inscan-reports
mkdir -p "$REPORTS_DIR"

echo "[$TIMESTAMP] Starting InSpec scan against fleet nodes" >> "$REPORTS_DIR/run.log"

for NODE_IP in 192.168.101.{211..213}; do
    echo "Scanning $NODE_IP..." >> "$REPORTS_DIR/run.log"
    
    # Run InSpec profiles from shared location
    for PROFILE in /tmp/spindle-qa/inspec/{web,database,loadbalancer}; do
        if [ -d "$PROFILE" ]; then
            ROLE=$(basename $(dirname "$PROFILE"))
            SSH_KEY="/home/operator/.ssh/id_ed25519_lab"
            RESULT=$(/usr/local/bin/inspec exec "$PROFILE" --input-file="$PROFILE/inputs.json" 2>&1 | tee "$REPORTS_DIR/${ROLE}-${NODE_IP}-${TIMESTAMP}.json" || true)
            
            STATUS="PASS"
            echo "$RESULT" | grep -q "Failed:" && STATUS="FAIL"
            echo "[$TIMESTAMP] ${ROLE}@${NODE_IP}: ${STATUS}" >> "$REPORTS_DIR/run.log"
        fi
    done
done

echo "[$TIMESTAMP] InSpec scan complete" >> "$REPORTS_DIR/run.log"
#!/bin/bash
#!/bin/bash
set -euo pipefail
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOGFILE=/var/log/spindle/cinc-converges/${TIMESTAMP}.log
mkdir -p /var/log/spindle/cinc-converges

for NODE_IP in 198.51.100.{211..213}; do
    echo "=== Cinc convergence on ${NODE_IP} at ${TIMESTAMP} ===" >> "$LOGFILE"
    
    # Local-mode converge against baseline cookbook
    if SSH_KEY="/home/operator/.ssh/id_ed25519_qemu_test" && sshpass -p ubuntu ssh -o StrictHostKeyChecking=no \
       -i "$SSH_KEY" ubuntu@"$NODE_IP" \
       "sudo cinc-client -z -l info -o json-pretty > /tmp/cinc-run-$(date +%s).json 2>&1; echo \$?" 2>/dev/null | tee -a "$LOGFILE"; then
        CONVERGE_EXIT=$(tail -1 "$LOGFILE")
        echo "[$TIMESTAMP] Converged on ${NODE_IP} (exit=${CONVERGE_EXIT})" >> "$LOGFILE"
    else
        echo "[$TIMESTAMP] Converge failed on ${NODE_IP} (SSH issue)" >> "$LOGFILE"
    fi
done

echo "[$TIMESTAMP] All converges complete" >> "$LOGFILE"
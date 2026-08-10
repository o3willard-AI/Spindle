#!/bin/bash
# provision-cinc-client.sh — wire a fleet node to the real Cinc server
# Usage: HOST=$HOST bash provision-cinc-client.sh  (run on the fleet node)
set -euo pipefail
NODE="$HOSTNAME"
CLIENT_KEY_PEM="/etc/cinc/${NODE}.pem"     # node's own client key
VALIDATOR="/etc/cinc/spindle-validator.pem" # org validator (for bootstrapping)

echo "=== provisioning $NODE to Cinc server 198.51.100.110 ==="

# 1. Install node client key (created server-side)
sudo install -d -o root -g root -m 0700 /etc/cinc
sudo install -m 0600 -o root -g root "/tmp/${NODE}.pem" "$CLIENT_KEY_PEM" 2>/dev/null || \
  sudo cp "/tmp/${NODE}.pem" "$CLIENT_KEY_PEM"
echo "[1] client key installed: $CLIENT_KEY_PEM"

# 2. Install validator key (copied from server /tmp/spindle-validator.pem)
#    (pulled via this script's caller when needed)
if [ -f /tmp/spindle-validator.pem ]; then
  sudo install -m 0600 -o root -g root /tmp/spindle-validator.pem "$VALIDATOR"
  echo "[2] validator key installed: $VALIDATOR"
else
  echo "[2] WARN: no validator key in /tmp — skipping (node auth via client key)"
fi

# 3. Trust the server's self-signed cert
sudo install -d -o root -g root -m 0700 /etc/cinc/trusted_certs
sudo cp /tmp/cinc-server.crt /etc/cinc/trusted_certs/cinc-server.crt
echo "[3] server cert trusted: /etc/cinc/trusted_certs/cinc-server.crt"

# 4. Rewrite client.rb for server-backed converge (keep data_collector twin-write)
cat > /tmp/client.rb <<'EOF'
log_level :info
log_location STDOUT

# Real Cinc Infra Server endpoint (org = spindle)
chef_server_url "https://198.51.100.110/organizations/spindle"
node_name "fleet-NODE"
client_key "/etc/cinc/fleet-NODE.pem"
validation_client_name "spindle-validator"
validation_key "/etc/cinc/spindle-validator.pem"
ssl_verify_mode :verify_none

# Twin-write shipping to the Spindle proxy (forwards to Spindle ingest + server)
data_collector['server_url'] = 'http://198.51.100.101:8081/ingest/events/data-collector'
data_collector['token'] = 'spindle-dev-token'
EOF
sed -i "s/fleet-NODE/${NODE}/g" /tmp/client.rb
sudo install -m 0644 -o root -g root /tmp/client.rb /etc/cinc/client.rb
echo "[4] client.rb written for $NODE -> server-backed"
echo "=== provisioned ==="
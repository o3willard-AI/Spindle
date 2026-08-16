#!/bin/bash
# deploy-qa-fleet.sh — Deploy Spindle QA cookbooks and roles to the QA fleet
#
# Usage:
#   QA_USER=ubuntu QA_KEY=~/.ssh/id_ed25519_lab bash deploy-qa-fleet.sh
#
# Node assignments:
#   fleet-01 (203.0.113.11) — spindle-web      (Apache + enterprise portal)
#   fleet-02 (203.0.113.12) — spindle-database  (PostgreSQL + tuning)
#   fleet-03 (203.0.113.13) — spindle-loadbalancer (HAProxy + SSL)

set -euo pipefail

QA_USER="${QA_USER:-ubuntu}"
QA_KEY="${QA_KEY:-$HOME/.ssh/id_ed25519_lab}"
COOKBOOK_DIR="$(cd "$(dirname "$0")/cookbooks" && pwd)"
ROLE_DIR="$(cd "$(dirname "$0")/roles" && pwd)"
INSPEC_DIR="$(cd "$(dirname "$0")/inspec" && pwd)"
DATA_COLLECTOR_URL="${DATA_COLLECTOR_URL:-http://192.0.2.10:8081/ingest/events/data-collector}"
SPINDLE_TOKEN="${SPINDLE_TOKEN:-spindle-dev-token}"
TMP_DIR="/tmp/spindle-qa-deploy-$$"

ssh_cmd() {
    ssh -i "$QA_KEY" -o StrictHostKeyChecking=no -o ConnectTimeout=10 "$QA_USER@$1" "$2"
}

scp_cmd() {
    scp -i "$QA_KEY" -o StrictHostKeyChecking=no -r "$1" "$QA_USER@$2:$3"
}

echo "=== Spindle QA Fleet Deployment ==="
echo "Data Collector URL: $DATA_COLLECTOR_URL"
echo ""

# ── Node 1: Web Server ─────────────────────────────────────────────────────
echo "--- fleet-01 (203.0.113.11): spindle-web ---"

ssh_cmd $2 "mkdir -p $TMP_DIR" && scp_cmd "$COOKBOOK_DIR" 203.0.113.11 "$TMP_DIR/"
scp_cmd "$ROLE_DIR/web.json" 203.0.113.11 "$TMP_DIR/role.json"
scp_cmd "$INSPEC_DIR/web" 203.0.113.11 "$TMP_DIR/inspec"

ssh_cmd 203.0.113.11 "
    # Install Cinc Client if missing
    if ! command -v cinc-client &>/dev/null; then
        curl -L https://omnitruck.cinc.sh/install.sh | sudo bash -s -- -v 18
    fi

    # Write client config with twin-write proxy
    sudo mkdir -p /etc/cinc
    sudo tee /etc/cinc/client.rb << 'CINC_CONFIG'
log_level :info
log_location STDOUT
data_collector['server_url'] = '${DATA_COLLECTOR_URL}'
data_collector['token'] = '${SPINDLE_TOKEN}'
CINC_CONFIG

    # Install cookbooks and converge
    sudo mkdir -p /var/chef/cookbooks
    sudo cp -r $TMP_DIR/cookbooks/* /var/chef/cookbooks/
    sudo cinc-client --local-mode --runlist 'recipe[apache2],recipe[apache2::mod_ssl],recipe[apache2::mod_headers],recipe[spindle-qa::web_app]' --override-runlist 'recipe[apache2],recipe[apache2::mod_ssl],recipe[apache2::mod_headers],recipe[spindle-qa::web_app]'

    # Run InSpec
    if command -v inspec &>/dev/null; then
        sudo inspec exec $TMP_DIR/inspec --reporter json > /tmp/inspec-web-report.json
        echo 'InSpec report saved to /tmp/inspec-web-report.json'
    fi

    rm -rf $TMP_DIR
    echo 'fleet-01 deploy complete'
"

echo ""

# ── Node 2: Database Server ──────────────────────────────────────────────────
echo "--- fleet-02 (203.0.113.12): spindle-database ---"

ssh_cmd $2 "mkdir -p $TMP_DIR" && scp_cmd "$COOKBOOK_DIR" 203.0.113.12 "$TMP_DIR/"
scp_cmd "$ROLE_DIR/database.json" 203.0.113.12 "$TMP_DIR/role.json"
scp_cmd "$INSPEC_DIR/database" 203.0.113.12 "$TMP_DIR/inspec"

ssh_cmd 203.0.113.12 "
    if ! command -v cinc-client &>/dev/null; then
        curl -L https://omnitruck.cinc.sh/install.sh | sudo bash -s -- -v 18
    fi

    sudo mkdir -p /etc/cinc
    sudo tee /etc/cinc/client.rb << 'CINC_CONFIG'
log_level :info
log_location STDOUT
data_collector['server_url'] = '${DATA_COLLECTOR_URL}'
data_collector['token'] = '${SPINDLE_TOKEN}'
CINC_CONFIG

    sudo mkdir -p /var/chef/cookbooks
    sudo cp -r $TMP_DIR/cookbooks/* /var/chef/cookbooks/
    sudo cinc-client --local-mode --runlist 'recipe[postgresql::server],recipe[spindle-qa::database]' --override-runlist 'recipe[postgresql::server],recipe[spindle-qa::database]'

    if command -v inspec &>/dev/null; then
        sudo inspec exec $TMP_DIR/inspec --reporter json > /tmp/inspec-db-report.json
        echo 'InSpec report saved to /tmp/inspec-db-report.json'
    fi

    rm -rf $TMP_DIR
    echo 'fleet-02 deploy complete'
"

echo ""

# ── Node 3: Load Balancer ────────────────────────────────────────────────────
echo "--- fleet-03 (203.0.113.13): spindle-loadbalancer ---"

ssh_cmd $2 "mkdir -p $TMP_DIR" && scp_cmd "$COOKBOOK_DIR" 203.0.113.13 "$TMP_DIR/"
scp_cmd "$ROLE_DIR/loadbalancer.json" 203.0.113.13 "$TMP_DIR/role.json"
scp_cmd "$INSPEC_DIR/loadbalancer" 203.0.113.13 "$TMP_DIR/inspec"

ssh_cmd 203.0.113.13 "
    if ! command -v cinc-client &>/dev/null; then
        curl -L https://omnitruck.cinc.sh/install.sh | sudo bash -s -- -v 18
    fi

    sudo mkdir -p /etc/cinc
    sudo tee /etc/cinc/client.rb << 'CINC_CONFIG'
log_level :info
log_location STDOUT
data_collector['server_url'] = '${DATA_COLLECTOR_URL}'
data_collector['token'] = '${SPINDLE_TOKEN}'
CINC_CONFIG

    sudo mkdir -p /var/chef/cookbooks
    sudo cp -r $TMP_DIR/cookbooks/* /var/chef/cookbooks/
    sudo cinc-client --local-mode --runlist 'recipe[haproxy],recipe[spindle-qa::loadbalancer]' --override-runlist 'recipe[haproxy],recipe[spindle-qa::loadbalancer]'

    if command -v inspec &>/dev/null; then
        sudo inspec exec $TMP_DIR/inspec --reporter json > /tmp/inspec-lb-report.json
        echo 'InSpec report saved to /tmp/inspec-lb-report.json'
    fi

    rm -rf $TMP_DIR
    echo 'fleet-03 deploy complete'
"

echo ""
echo "=== Deployment Complete ==="
echo ""
echo "Data flowing to: $DATA_COLLECTOR_URL"
echo "Monitor: curl -s http://192.0.2.10:8081/health"
echo ""
echo "Next steps:"
echo "  1. Verify twin-write health dashboard"
echo "  2. Run 'sudo cinc-client --once' on each node for additional converges"
echo "  3. Run 'sudo inspec exec /tmp/spindle-qa-deploy-*/inspec --reporter json' for compliance scans"
echo "  4. When confident, update client.rb to point directly at Spindle (no proxy)"

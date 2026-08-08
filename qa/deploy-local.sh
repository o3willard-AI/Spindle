#!/bin/bash
set -euo pipefail

NODE_NAME="$1"
RECIPES="$2"

echo "--- Deploying ${NODE_NAME} ---"

# Install cookbooks locally
sudo mkdir -p /var/chef/cookbooks
sudo cp -r /tmp/spindle-deploy/spindle-qa/* /var/chef/cookbooks/

echo "Cookbooks installed:"
ls /var/chef/cookbooks/

# Run converge
echo "Running converge: ${RECIPES}"
sudo cinc-client --local-mode \
    --runlist "${RECIPES}" \
    --override-runlist "${RECIPES}" 2>&1 | tail -30

echo "--- Converge done for ${NODE_NAME} ---"

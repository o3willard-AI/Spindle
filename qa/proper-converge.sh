#!/bin/bash
# Proper local-mode converge using cinc-client -z with explicit cookbook path.
# This avoids omnitruck/Supermarket entirely.
set -euo pipefail

echo "--- Fixing metadata.rb to be fully self-contained ---"

METADATA="/var/chef/cookbooks/spindle-qa/metadata.rb"

# Add dependencies section header if missing
if ! grep -q '^dependencies' "$METADATA" 2>/dev/null; then
    echo "Adding empty dependencies block to $METADATA"
    sed -i '$ a\
dependencies {}' "$METADATA"
fi

echo "--- Running cinc-client in zero-mode (no server) ---"

# Zero mode (-z) = no chef-server contact
# Explicit cookbook_path ensures we use /var/chef/cookbooks, not supermarket
sudo cinc-client \
    -z \
    --runlist "recipe[spindle-qa::$RECIPE]" \
    --config-option "cookbook_path=/var/chef/cookbooks" \
    2>&1

echo ""
echo "--- Convergence complete for recipe[$RECIPE] ---"

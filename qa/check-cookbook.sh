#!/bin/bash
# Check what's installed on fleet nodes
echo "=== Packages ==="
dpkg -l 2>/dev/null | grep -E "apache2|postgresql|haproxy" | awk '{print $2, $3}'

echo ""
echo "=== Cookbook tree ==="
find /var/chef/cookbooks/spindle-qa -type f 2>/dev/null || echo "NO SPINDLE-QA"

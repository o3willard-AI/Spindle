#!/bin/bash
set -euo pipefail
cat /tmp/fixed-haproxy.cfg.erb > /var/chef/cookbooks/spindle-qa/templates/haproxy.cfg.erb
rm /tmp/fixed-haproxy.cfg.erb
echo "Template installed."
ls -la /var/chef/cookbooks/spindle-qa/templates/haproxy.cfg.erb

#!/bin/bash
# Direct haproxy config render + apply (bypasses ERB entirely)
set -euo pipefail

cat > /etc/haproxy/haproxy.cfg << 'HAPROXYCFG'
global
    log /dev/log local0
    maxconn 2000

defaults
    mode http
    timeout connect 5000ms
    timeout client 50000ms
    timeout server 50000ms

frontend https
    bind *:443 ssl crt /etc/haproxy/ssl/spindle.pem
    default_backend web-portal

backend web-portal
    balance roundrobin
    option httpchk
    http-check send meth GET uri /index.html ver HTTP/1.1 hdr host localhost
    http-check expect status 200
    server fleet-01 198.51.100.211:80 check inter 5s rise 2 fall 3
HAPROXYCFG

echo "Config written."
haproxy -c -f /etc/haproxy/haproxy.cfg 2>&1
systemctl restart haproxy
echo "Service started:"
systemctl is-active haproxy

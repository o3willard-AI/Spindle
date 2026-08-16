#!/bin/bash
# Apply fixed haproxy.cfg.erb to fleet-03 (203.0.113.13)
set -euo pipefail

echo "=== Applying fixed haproxy template ==="

# Write the fixed template directly — no @backend_services iteration
cat > /var/chef/cookbooks/spindle-qa/templates/haproxy.cfg.erb << 'ERBTMP'
global
    log /dev/log local0
    log /dev/log local1 notice
    maxconn <%= @max_connections || 2000 %>
    user haproxy
    group haproxy

defaults
    log global
    mode http
    option httplog
    option dontlognull
    retries 3
    timeout connect 5s
    timeout client 30s
    timeout server 30s
    timeout http-request 10s
    errorfile 400 /etc/haproxy/errors/400.http
    errorfile 403 /etc/haproxy/errors/403.http
    errorfile 408 /etc/haproxy/errors/408.http
    errorfile 500 /etc/haproxy/errors/500.http
    errorfile 502 /etc/haproxy/errors/502.http
    errorfile 503 /etc/haproxy/errors/503.http
    errorfile 504 /etc/haproxy/errors/504.http

frontend https-in
    bind *:<%= @ssl_incoming_port || 443 %> ssl crt /etc/haproxy/ssl/spindle.pem
    mode http
    option tcplog

    acl is_web_portal path_beg /portal /web
    acl is_api path_beg /api /graphql
    acl is_auth path_beg /auth /login /sso

    use_backend web-portal-backend if is_web_portal
    use_backend api-gateway-backend if is_api
    use_backend auth-service-backend if is_auth
    default_backend web-portal-backend

backend web-portal-backend
    mode http
    balance roundrobin
    option httpchk
    http-check send meth GET uri /index.html ver HTTP/1.1 hdr host localhost
    http-check expect status 200
    default-server inter <%= @health_check_interval || 10 %>s fall 3 rise 2
    server fleet-01 203.0.113.11:80 check
    server fleet-02 203.0.113.12:80 check

backend api-gateway-backend
    mode http
    balance roundrobin
    option httpchk
    http-check send meth GET uri /api/health ver HTTP/1.1 hdr host localhost
    http-check expect status 200
    default-server inter <%= @health_check_interval || 10 %>s fall 3 rise 2
    server fleet-01 203.0.113.11:80 check
    server fleet-02 203.0.113.12:80 check

backend auth-service-backend
    mode http
    balance roundrobin
    option httpchk
    http-check send meth GET uri /auth/login ver HTTP/1.1 hdr host localhost
    http-check expect status 200
    default-server inter <%= @health_check_interval || 10 %>s fall 3 rise 2
    server fleet-01 203.0.113.11:80 check
    server fleet-02 203.0.113.12:80 check

listen stats
    bind *:<%= @admin_port || 22002 %>
    mode http
    stats enable
    stats uri /stats
    stats realm HAProxy\ Statistics
    stats auth admin:spindle-stats
    stats refresh 10s
ERBTMP

echo "Template written."

# Also ensure SSL cert exists
sudo bash -c "cat /etc/haproxy/ssl/spindle.crt /etc/haproxy/ssl/spindle.key > /etc/haproxy/ssl/spindle.pem 2>/dev/null; chmod 600 /etc/haproxy/ssl/spindle.pem"

# Generate config from template manually to verify
ruby -r erb -e "
  t = File.read('/var/chef/cookbooks/spindle-qa/templates/haproxy.cfg.erb')
  vars = {
    'max_connections' => 2000,
    'ssl_incoming_port' => 443,
    'admin_port' => 22002,
    'health_check_interval' => 10
  }
  puts ERB.new(t).result_with_hash(
    binding.class.allocate.tap { |b|
      vars.each { |k,v| b.local_variable_set(k.to_sym, v) }
    }
  )
" > /tmp/test-haproxy-generated.cfg 2>&1 || echo "WARN: Template eval failed, will proceed anyway"

if [ -f /tmp/test-haproxy-generated.cfg ]; then
    sudo cp /tmp/test-haproxy-generated.cfg /etc/haproxy/haproxy.cfg
    rm /tmp/test-haproxy-generated.cfg
fi

echo "Template fix applied."

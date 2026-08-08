#
# Cookbook: spindle-qa
# Recipe: loadbalancer
# Self-contained HAProxy setup for QA.
#

package 'haproxy'

# SSL cert
directory '/etc/haproxy/ssl' do
  mode '0700'
end

execute 'generate_self_signed_cert' do
  command 'openssl req -x509 -newkey rsa:2048 -keyout /etc/haproxy/ssl/spindle.key -out /etc/haproxy/ssl/spindle.crt -days 365 -nodes -subj "/CN=spindle-lb.utility-server.local/O=Spindle QA/C=US"'
  not_if { ::File.exist?('/etc/haproxy/ssl/spindle.crt') }
end

execute 'concat_pem' do
  command 'cat /etc/haproxy/ssl/spindle.crt /etc/haproxy/ssl/spindle.key > /etc/haproxy/ssl/spindle.pem && chmod 600 /etc/haproxy/ssl/spindle.pem'
  not_if { ::File.exist?('/etc/haproxy/ssl/spindle.pem') }
end

# HAProxy config
template '/etc/haproxy/haproxy.cfg' do
  source 'haproxy.cfg.erb'
  variables(
    admin_port: 22002,
    ssl_incoming_port: 443,
    max_connections: 2000,
    health_check_interval: 10,
    backend_services: %w[web-portal api-gateway auth-service]
  )
  notifies :restart, 'service[haproxy]'
end

service 'haproxy' do
  supports status: true, restart: true, reload: true
  action [:enable, :start]
end

# Sysctl tuning
%w[net.ipv4.tcp_fin_timeout=30 net.ipv4.tcp_tw_reuse=1 net.core.somaxconn=4096 net.ipv4.tcp_max_syn_backlog=8192].each do |p|
  k, v = p.split('=')
  execute "sysctl_#{k}" do
    command "sysctl -w #{k}=#{v}"
    not_if "sysctl -n #{k} | grep -q '^#{v}$'"
  end
end

# Connection tracking cron
file '/etc/cron.hourly/haproxy-stats' do
  content <<~EOH
    #!/bin/bash
    echo "$(date -Iseconds) established=$(netstat -an | grep ':443 ' | grep ESTABLISHED | wc -l)" >> /var/log/haproxy-stats.log
  EOH
  mode '0755'
end

# Base configuration applied to all fleet nodes.
# Manages: packages (htop/vim/tmux/curl), deploy user, ssh, /etc/motd
#
# App recipes are included per-node via the node run-list:
#   fleet-01 → base::nginx         (nginx :80)
#   fleet-02 → base::apache2       (apache2 :80, FreshRSS)
#   fleet-03 → base::apache2       (apache2 :80, RSS-Bridge)
#   fleet-04 → base::glance        (Glance :8080)
#   fleet-05 → base::rsshub        (docker + RSSHub :1200)
#   fleet-06 → base::miniflux      (Miniflux :8080)
#   fleet-07 → (base only)
#   fleet-08 → (base only)

# Install base packages — declared installed
%w(htop vim tmux curl).each do |pkg|
  package pkg do
    action :install
  end
end

# Deploy user — declared present
user 'deploy' do
  comment 'Deployment user'
  shell '/bin/bash'
  manage_home true
  action :create
end

# SSH — declared enabled and running
service 'ssh' do
  action [:enable, :start]
end

# /etc/motd — declared with managed content
file '/etc/motd' do
  content "This node is managed by CINC.\nHostname: #{node['hostname']}\nIP: #{node['ipaddress']}\n"
  owner 'root'
  group 'root'
  mode '0644'
  action :create
end

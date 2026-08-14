# Base configuration applied to all fleet nodes.
# Installs packages, creates deploy user, enables SSH, writes MOTD.

node['base']['packages'].each do |pkg|
  package pkg do
    action :install
  end
end

user node['base']['deploy_user'] do
  comment 'Deployment user'
  shell '/bin/bash'
  manage_home true
end

service 'ssh' do
  action [:enable, :start]
end

file node['base']['motd']['path'] do
  content "This node is managed by CINC.\nHostname: #{node['hostname']}\nIP: #{node['ipaddress']}\n"
  owner node['base']['motd']['owner']
  group node['base']['motd']['group']
  mode node['base']['motd']['mode']
end

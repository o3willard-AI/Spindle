# Glance dashboard (fleet-04) — listen port :8080
# Declared: installed, enabled, running, config content, listen port

# Config directory
directory '/home/ubuntu/.glance' do
  owner 'ubuntu'
  group 'ubuntu'
  mode '0755'
  action :create
end

# Glance binary — declared installed
remote_file '/tmp/glance.tar.gz' do
  source 'https://github.com/glanceapp/glance/releases/download/v0.8.5/glance-linux-amd64.tar.gz'
  not_if { ::File.exist?('/usr/local/bin/glance') }
  action :create
end

execute 'extract glance' do
  command 'tar -xzf /tmp/glance.tar.gz -C /tmp && mv /tmp/glance /usr/local/bin/glance && chmod +x /usr/local/bin/glance'
  not_if { ::File.exist?('/usr/local/bin/glance') }
  action :run
end

# Glance config — declared with managed content
cookbook_file '/home/ubuntu/.glance/glance.yml' do
  source 'glance/glance.yml'
  owner 'ubuntu'
  group 'ubuntu'
  mode '0644'
  notifies :restart, 'service[glance]'
end

# Systemd unit — declared with managed content
template '/etc/systemd/system/glance.service' do
  source 'glance.service.erb'
  owner 'root'
  group 'root'
  mode '0644'
  notifies :run, 'execute[reload systemd]', :immediately
end

execute 'reload systemd' do
  command 'systemctl daemon-reload'
  action :nothing
end

# Glance service — declared enabled and running, listen port :8080
service 'glance' do
  action [:enable, :start]
  supports status: true, restart: true
  subscribes :restart, 'template[/etc/systemd/system/glance.service]'
  subscribes :restart, 'cookbook_file[/home/ubuntu/.glance/glance.yml]'
end

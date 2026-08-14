# Miniflux RSS reader (fleet-06) — listen port :8080
# Declared: installed, enabled, running, config content, listen port

# Miniflux binary — declared installed
remote_file '/tmp/miniflux' do
  source 'https://github.com/miniflux/v2/releases/download/2.3.3/miniflux-linux-amd64'
  not_if { ::File.exist?('/usr/local/bin/miniflux') }
  mode '0755'
end

execute 'install miniflux' do
  command 'mv /tmp/miniflux /usr/local/bin/miniflux && chmod +x /usr/local/bin/miniflux'
  not_if { ::File.exist?('/usr/local/bin/miniflux') }
end

# Config directory
directory '/etc/miniflux' do
  owner 'root'
  group 'root'
  mode '0755'
end

# Miniflux config — declared with managed content
file '/etc/miniflux/miniflux.conf' do
  content <<~CONF
    DATABASE_URL=postgres://miniflux:miniflux-secret@192.168.100.12:5432/miniflux?sslmode=disable
    LISTEN_ADDR=0.0.0.0:8080
    FETCHER_ALLOW_PRIVATE_NETWORKS=1
  CONF
  owner 'root'
  group 'root'
  mode '0600'
  notifies :restart, 'service[miniflux]'
end

# Systemd unit — declared with managed content
template '/etc/systemd/system/miniflux.service' do
  source 'miniflux.service.erb'
  owner 'root'
  group 'root'
  mode '0644'
  notifies :run, 'execute[reload systemd miniflux]', :immediately
end

execute 'reload systemd miniflux' do
  command 'systemctl daemon-reload'
  action :nothing
end

# Miniflux service — declared enabled and running, listen port :8080
service 'miniflux' do
  action [:enable, :start]
  supports status: true, restart: true
  subscribes :restart, 'template[/etc/systemd/system/miniflux.service]'
  subscribes :restart, 'file[/etc/miniflux/miniflux.conf]'
end

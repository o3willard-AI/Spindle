# Miniflux RSS reader (fleet-06): single binary + PostgreSQL.
# Provides a self-hosted RSS reader with feed fetching and sharing.

miniflux_version = node['base']['miniflux']['version']
miniflux_user = node['base']['miniflux']['user']
miniflux_config_path = node['base']['miniflux']['config_path']
miniflux_db_url = node['base']['miniflux']['database_url']
miniflux_port = node['base']['miniflux']['listen_port']
miniflux_config_dir = ::File.dirname(miniflux_config_path)

remote_file '/tmp/miniflux' do
  source "https://github.com/miniflux/v2/releases/download/#{miniflux_version}/miniflux-linux-amd64"
  not_if { ::File.exist?('/usr/local/bin/miniflux') }
end

execute 'install miniflux' do
  command 'mv /tmp/miniflux /usr/local/bin/miniflux && chmod +x /usr/local/bin/miniflux'
  not_if { ::File.exist?('/usr/local/bin/miniflux') }
end

directory miniflux_config_dir do
  owner 'root'
  group 'root'
  mode '0755'
end

file miniflux_config_path do
  content <<~CONF
    DATABASE_URL=#{miniflux_db_url}
    LISTEN_ADDR=0.0.0.0:#{miniflux_port}
    FETCHER_ALLOW_PRIVATE_NETWORKS=1
  CONF
  owner 'root'
  group miniflux_user
  mode '0640'
  notifies :restart, 'service[miniflux]'
end

systemd_unit 'miniflux.service' do
  content <<~UNIT
    [Unit]
    Description=Miniflux
    After=network.target

    [Service]
    ExecStart=/usr/local/bin/miniflux -c #{miniflux_config_path}
    Restart=always
    User=#{miniflux_user}

    [Install]
    WantedBy=multi-user.target
  UNIT
  action :create
end

service 'miniflux' do
  action [:enable, :start]
end

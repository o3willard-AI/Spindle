# Glance dashboard (fleet-04): single Go binary + systemd unit + config.
# Provides weather, markets, HN, Reddit, and RSS widgets in a single page.

glance_user = node['base']['glance']['user']
glance_config_dir = node['base']['glance']['config_dir']
glance_version = node['base']['glance']['version']

directory glance_config_dir do
  owner glance_user
  group glance_user
  mode '0755'
end

remote_file '/tmp/glance.tar.gz' do
  source "https://github.com/glanceapp/glance/releases/download/v#{glance_version}/glance-linux-amd64.tar.gz"
  not_if { ::File.exist?('/usr/local/bin/glance') }
end

execute 'extract glance' do
  command 'tar -xzf /tmp/glance.tar.gz -C /tmp && mv /tmp/glance /usr/local/bin/glance && chmod +x /usr/local/bin/glance'
  not_if { ::File.exist?('/usr/local/bin/glance') }
end

cookbook_file "#{glance_config_dir}/glance.yml" do
  source 'glance/glance.yml'
  owner glance_user
  group glance_user
  mode '0644'
  notifies :restart, 'service[glance]'
end

systemd_unit 'glance.service' do
  content <<~UNIT
    [Unit]
    Description=Glance Dashboard
    After=network.target

    [Service]
    ExecStart=/usr/local/bin/glance --config #{glance_config_dir}/glance.yml
    Restart=always
    User=#{glance_user}

    [Install]
    WantedBy=multi-user.target
  UNIT
  action :create
end

service 'glance' do
  action [:enable, :start]
end

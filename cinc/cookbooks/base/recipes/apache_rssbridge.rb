# Apache + PHP + RSS-Bridge (fleet-03).
# Installs Apache2 with PHP modules, downloads and configures RSS-Bridge.

node['base']['apache2']['php_packages'].each do |pkg|
  package pkg
end

# Stop Nginx if running — port 80 conflict
service 'nginx' do
  action [:disable, :stop]
  only_if { ::File.exist?('/lib/systemd/system/nginx.service') || ::File.exist?('/etc/systemd/system/nginx.service') }
  ignore_failure true
end

directory node['base']['apache2']['rssbridge']['doc_root'] do
  owner 'www-data'
  group 'www-data'
  mode '0755'
  recursive true
end

rssbridge_version = node['base']['apache2']['rssbridge']['version']
remote_file '/tmp/rss-bridge.tar.gz' do
  source "https://github.com/RSS-Bridge/rss-bridge/archive/refs/tags/#{rssbridge_version}.tar.gz"
  not_if { ::File.exist?("#{node['base']['apache2']['rssbridge']['doc_root']}/index.php") }
end

execute 'extract rss-bridge' do
  command "tar -xzf /tmp/rss-bridge.tar.gz -C #{node['base']['apache2']['rssbridge']['doc_root']} --strip-components=1"
  not_if { ::File.exist?("#{node['base']['apache2']['rssbridge']['doc_root']}/index.php") }
  notifies :run, 'execute[rss-bridge perms]', :immediately
end

execute 'rss-bridge perms' do
  command "chown -R www-data:www-data #{node['base']['apache2']['rssbridge']['doc_root']}"
  action :nothing
end

template '/etc/apache2/sites-available/rss-bridge.conf' do
  source 'apache-rssbridge-vhost.conf.erb'
  owner 'root'
  group 'root'
  mode '0644'
  variables(
    server_name: node['hostname'],
    admin_email: node['base']['apache2']['rssbridge']['admin_email'],
    doc_root: node['base']['apache2']['rssbridge']['doc_root']
  )
  notifies :reload, 'service[apache2]'
end

execute 'enable rss-bridge site' do
  command 'a2ensite rss-bridge.conf'
  not_if { ::File.exist?('/etc/apache2/sites-enabled/rss-bridge.conf') }
  notifies :reload, 'service[apache2]'
end

service 'apache2' do
  action [:enable, :start]
  supports status: true, restart: true, reload: true
end

# Apache + PHP + FreshRSS RSS reader (fleet-02).
# Installs Apache2 with PHP modules, downloads and configures FreshRSS.

node['base']['apache2']['php_packages'].each do |pkg|
  package pkg
end

# Stop Nginx if running — port 80 conflict
service 'nginx' do
  action [:disable, :stop]
  only_if { ::File.exist?('/lib/systemd/system/nginx.service') || ::File.exist?('/etc/systemd/system/nginx.service') }
  ignore_failure true
end

execute 'enable apache rewrite' do
  command 'a2enmod rewrite'
  not_if { ::File.exist?('/etc/apache2/mods-enabled/rewrite.load') }
  notifies :restart, 'service[apache2]'
end

directory node['base']['apache2']['freshrss']['doc_root'] do
  owner 'www-data'
  group 'www-data'
  mode '0755'
  recursive true
end

freshrss_version = node['base']['apache2']['freshrss']['version']
remote_file '/tmp/freshrss.tar.gz' do
  source "https://github.com/FreshRSS/FreshRSS/archive/refs/tags/#{freshrss_version}.tar.gz"
  not_if { ::File.exist?("#{node['base']['apache2']['freshrss']['doc_root']}/index.php") }
end

execute 'extract freshrss' do
  command "tar -xzf /tmp/freshrss.tar.gz -C #{node['base']['apache2']['freshrss']['doc_root']} --strip-components=1"
  not_if { ::File.exist?("#{node['base']['apache2']['freshrss']['doc_root']}/index.php") }
  notifies :run, 'execute[freshrss perms]', :immediately
end

execute 'freshrss perms' do
  command "chown -R www-data:www-data #{node['base']['apache2']['freshrss']['doc_root']} && chmod -R g+w #{node['base']['apache2']['freshrss']['doc_root']}/data"
  action :nothing
end

template '/etc/apache2/sites-available/freshrss.conf' do
  source 'apache-freshrss-vhost.conf.erb'
  owner 'root'
  group 'root'
  mode '0644'
  variables(
    server_name: node['hostname'],
    admin_email: node['base']['apache2']['freshrss']['admin_email'],
    doc_root: node['base']['apache2']['freshrss']['doc_root']
  )
  notifies :reload, 'service[apache2]'
end

execute 'enable freshrss site' do
  command 'a2ensite freshrss.conf'
  not_if { ::File.exist?('/etc/apache2/sites-enabled/freshrss.conf') }
  notifies :reload, 'service[apache2]'
end

service 'apache2' do
  action [:enable, :start]
  supports status: true, restart: true, reload: true
end

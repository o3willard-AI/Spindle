# Nginx landing page for the fleet (fleet-01).
# Provides a fleet services index page on port 80.

package 'nginx'

# Stop Apache2 if running — port 80 conflict
service 'apache2' do
  action [:disable, :stop]
  only_if { ::File.exist?('/etc/init.d/apache2') || ::File.exist?('/etc/systemd/system/apache2.service') || ::File.exist?('/lib/systemd/system/apache2.service') }
  ignore_failure true
end

cookbook_file '/var/www/html/index.html' do
  source 'nginx/index.html'
  owner node['base']['nginx']['user']
  group node['base']['nginx']['user']
  mode '0644'
  notifies :reload, 'service[nginx]'
end

template '/etc/nginx/nginx.conf' do
  source 'nginx.conf.erb'
  owner 'root'
  group 'root'
  mode '0644'
  variables(
    server_name: node['hostname'],
    listen_port: node['base']['nginx']['listen_port'],
    nginx_user: node['base']['nginx']['user']
  )
  notifies :reload, 'service[nginx]'
end

service 'nginx' do
  action [:enable, :start]
  supports status: true, restart: true, reload: true
end

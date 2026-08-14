# Nginx web server (fleet-01) — listen port :80
# Declared: installed, enabled, running, config content, listen port

# Install nginx — declared installed
package 'nginx' do
  action :install
end

# Nginx config — declared with managed content
template '/etc/nginx/nginx.conf' do
  source 'nginx.conf.erb'
  owner 'root'
  group 'root'
  mode '0644'
  variables(
    server_name: node['hostname']
  )
  notifies :reload, 'service[nginx]'
end

# Index page — declared with managed content
cookbook_file '/var/www/html/index.html' do
  source 'nginx/index.html'
  owner 'www-data'
  group 'www-data'
  mode '0644'
  notifies :reload, 'service[nginx]'
end

# Nginx service — declared enabled and running
service 'nginx' do
  action [:enable, :start]
  supports status: true, restart: true, reload: true
end

# Apache2 web server (fleet-02, fleet-03) — listen port :80
# Declared: installed, enabled, running, config content, listen port
#
# fleet-02: FreshRSS application
# fleet-03: RSS-Bridge application

# Install Apache + PHP — declared installed
%w(
  apache2
  php
  libapache2-mod-php
  php-xml
  php-curl
  php-mbstring
  php-json
  php-intl
  php-zip
  php-gd
  php-sqlite3
).each do |pkg|
  package pkg do
    action :install
  end
end

# Enable Apache rewrite module
execute 'enable apache rewrite' do
  command 'a2enmod rewrite'
  not_if { ::File.exist?('/etc/apache2/mods-enabled/rewrite.load') }
  notifies :restart, 'service[apache2]'
end

# Apache virtual host — declared with managed content
template '/etc/apache2/sites-available/fleet-app.conf' do
  source 'apache-vhost.conf.erb'
  owner 'root'
  group 'root'
  mode '0644'
  variables(
    server_name: node['hostname'],
    server_admin: 'ops@fleet.example',
    doc_root: '/var/www/html'
  )
  notifies :reload, 'service[apache2]'
end

# Enable our site
execute 'enable fleet app site' do
  command 'a2ensite fleet-app.conf'
  not_if { ::File.exist?('/etc/apache2/sites-enabled/fleet-app.conf') }
  notifies :reload, 'service[apache2]'
end

# Disable default site if present
execute 'disable default site' do
  command 'a2dissite 000-default'
  only_if { ::File.exist?('/etc/apache2/sites-enabled/000-default.conf') }
  notifies :reload, 'service[apache2]'
end

# Document root
directory '/var/www/html' do
  owner 'www-data'
  group 'www-data'
  mode '0755'
  action :create
end

# Apache service — declared enabled and running
service 'apache2' do
  action [:enable, :start]
  supports status: true, restart: true, reload: true
end

#
# Cookbook: spindle-qa
# Recipe: web_app
# Self-contained — no supermarket dependencies. Installs and configures
# Apache 2.4 directly for QA pipeline validation.
#

# Ensure spindle_qa attributes exist
node.default["spindle_qa"] = {} unless node["spindle_qa"]

# Install Apache
package 'apache2'

# Enable required modules
%w[rewrite ssl headers].each do |mod|
  execute "a2enmod #{mod}" do
    command "/usr/sbin/a2enmod #{mod}"
    not_if { ::File.exist?("/etc/apache2/mods-enabled/#{mod}.load") }
    notifies :restart, 'service[apache2]'
  end
end

# Application directory
directory '/var/www/spindle-enterprise-portal' do
  owner 'www-data'
  group 'www-data'
  mode '0755'
  recursive true
end

# Deploy index page
template '/var/www/spindle-enterprise-portal/index.html' do
  source 'index.html.erb'
  owner 'www-data'
  group 'www-data'
  mode '0644'
  variables(
    app_name: node['spindle_qa']['app_name'] || 'spindle-enterprise-portal',
    deploy_time: Time.now.utc.iso8601
  )
end

# Department portals
%w[engineering finance marketing operations].each do |dept|
  directory "/var/www/spindle-enterprise-portal/#{dept}" do
    owner 'www-data'
    group 'www-data'
    mode '0755'
  end

  file "/var/www/spindle-enterprise-portal/#{dept}/index.html" do
    content "<html><body><h1>#{dept.capitalize} Portal</h1><p>Spindle QA — #{dept}</p><p>Updated: #{Time.now.utc.iso8601}</p></body></html>"
    owner 'www-data'
    group 'www-data'
    mode '0644'
  end
end

# Virtual host
template '/etc/apache2/sites-available/spindle-enterprise.conf' do
  source 'apache-vhost.conf.erb'
  owner 'root'
  group 'root'
  mode '0644'
  variables(
    server_name: node['hostname'],
    server_admin: 'ops@spindle.dev',
    app_root: '/var/www/spindle-enterprise-portal'
  )
  notifies :reload, 'service[apache2]'
end

# Listen port (restore/ensure 80) — chaos may set a non-standard port
template '/etc/apache2/ports.conf' do
  source 'apache-ports.conf.erb'
  owner 'root'
  group 'root'
  mode '0644'
  notifies :reload, 'service[apache2]'
end

# Disable default site, enable ours
execute 'a2dissite 000-default' do
  command '/usr/sbin/a2dissite 000-default'
  only_if { ::File.exist?('/etc/apache2/sites-enabled/000-default.conf') }
  notifies :reload, 'service[apache2]'
end

execute 'a2ensite spindle-enterprise' do
  command '/usr/sbin/a2ensite spindle-enterprise'
  not_if { ::File.exist?('/etc/apache2/sites-enabled/spindle-enterprise.conf') }
  notifies :reload, 'service[apache2]'
end

# Security headers file
file '/etc/apache2/conf-available/security-headers.conf' do
  content <<~EOH
    Header always set X-Content-Type-Options "nosniff"
    Header always set X-Frame-Options "SAMEORIGIN"
    Header always set X-XSS-Protection "1; mode=block"
    Header always set Referrer-Policy "strict-origin-when-cross-origin"
  EOH
  mode '0644'
  notifies :reload, 'service[apache2]'
end

execute 'a2enconf security-headers' do
  command '/usr/sbin/a2enconf security-headers'
  not_if { ::File.exist?('/etc/apache2/conf-enabled/security-headers.conf') }
  notifies :reload, 'service[apache2]'
end

# Error pages
%w[403 404 500 502 503].each do |code|
  file "/var/www/spindle-enterprise-portal/#{code}.html" do
    content "<html><body><h1>Error #{code}</h1><p>Spindle Enterprise Portal</p></body></html>"
    owner 'www-data'
    group 'www-data'
    mode '0644'
  end
end

# Custom log directory
directory '/var/log/apache2' do
  mode '0755'
end

file '/etc/logrotate.d/spindle-apache' do
  content <<~EOH
    /var/log/apache2/spindle-*.log {
        daily
        rotate 30
        compress
        missingok
        notifempty
        sharedscripts
        postrotate
            /usr/sbin/apache2ctl graceful > /dev/null 2>&1
        endscript
    }
  EOH
  mode '0644'
end

service 'apache2' do
  supports status: true, restart: true, reload: true
  action [:enable, :start]
end

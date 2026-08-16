# Default attributes for the base cookbook.
# Override per-environment via role attributes or cinc-client -j.

# Base packages installed on every fleet node
default['base']['packages'] = %w(htop vim tmux curl)

# Deploy user
default['base']['deploy_user'] = 'deploy'

# Nginx (fleet-01)
default['base']['nginx']['listen_port'] = 80
default['base']['nginx']['user'] = 'www-data'

# Apache2 (fleet-02 FreshRSS, fleet-03 RSS-Bridge)
default['base']['apache2']['listen_port'] = 80
default['base']['apache2']['php_packages'] = %w(
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
)

# Apache2 FreshRSS (fleet-02)
default['base']['apache2']['freshrss']['version'] = '1.29.1'
default['base']['apache2']['freshrss']['doc_root'] = '/var/www/html/freshrss'
default['base']['apache2']['freshrss']['admin_email'] = 'ops@fleet.example'

# Apache2 RSS-Bridge (fleet-03)
default['base']['apache2']['rssbridge']['version'] = '2025-08-05'
default['base']['apache2']['rssbridge']['doc_root'] = '/var/www/html/rss-bridge'
default['base']['apache2']['rssbridge']['admin_email'] = 'ops@fleet.example'

# Glance (fleet-04)
default['base']['glance']['version'] = '0.8.5'
default['base']['glance']['listen_port'] = 8080
default['base']['glance']['config_dir'] = '/home/ubuntu/.glance'
default['base']['glance']['user'] = 'ubuntu'

# RSSHub (fleet-05)
default['base']['rsshub']['listen_port'] = 1200
default['base']['rsshub']['image'] = 'diygod/rsshub'
default['base']['rsshub']['redis_image'] = 'redis:alpine'

# Miniflux (fleet-06)
default['base']['miniflux']['version'] = '2.3.3'
default['base']['miniflux']['listen_port'] = 8080
default['base']['miniflux']['database_url'] = 'postgres://miniflux:CHANGE_ME@198.51.100.12:5432/miniflux?sslmode=disable'
default['base']['miniflux']['user'] = 'ubuntu'
default['base']['miniflux']['config_path'] = '/etc/miniflux/miniflux.conf'

# Motd
default['base']['motd']['path'] = '/etc/motd'
default['base']['motd']['owner'] = 'root'
default['base']['motd']['group'] = 'root'
default['base']['motd']['mode'] = '0644'

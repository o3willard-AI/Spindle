# Default attributes for the base cookbook.
# Override per-environment via role attributes or chef-client -j.

# Base packages
default['base']['packages'] = %w(htop vim tmux curl)

# Deploy user
default['base']['deploy_user'] = 'deploy'

# Nginx (fleet-01)
default['base']['nginx']['listen_port'] = 80

# Apache2 (fleet-02, fleet-03)
default['base']['apache2']['listen_port'] = 80

# Glance (fleet-04)
default['base']['glance']['version'] = '0.8.5'
default['base']['glance']['listen_port'] = 8080
default['base']['glance']['config_dir'] = '/home/ubuntu/.glance'
default['base']['glance']['user'] = 'ubuntu'

# Miniflux (fleet-06)
default['base']['miniflux']['version'] = '2.3.3'
default['base']['miniflux']['listen_port'] = 8080
default['base']['miniflux']['database_url'] = 'postgres://miniflux:miniflux-secret@192.168.100.12:5432/miniflux?sslmode=disable'

# RSSHub (fleet-05)
default['base']['rsshub']['listen_port'] = 1200

# Apache2 PHP packages
default['base']['apache2']['php_packages'] = %w(
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

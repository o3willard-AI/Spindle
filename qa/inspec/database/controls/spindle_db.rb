# InSpec profile: spindle-database
# Wraps dev-sec/postgres-baseline with Spindle-specific controls
name 'spindle-database'
title 'Spindle QA — Database Server Compliance'
maintainer 'Spindle QA Team'
copyright 'Spindle QA Team'
license 'MIT'
version '1.0.0'
supports platform: 'ubuntu'

depends 'postgres-baseline', url: 'https://github.com/dev-sec/postgres-baseline/archive/master.tar.gz'

control 'spindle-db-01' do
  impact 0.9
  title 'PostgreSQL service must be running'
  describe service('postgresql') do
    it { should be_running }
    it { should be_enabled }
  end
end

control 'spindle-db-02' do
  impact 0.7
  title 'All application databases must exist'
  %w[spindle_production spindle_staging spindle_analytics].each do |db|
    describe command("sudo -u postgres psql -t -c \"SELECT 1 FROM pg_database WHERE datname='#{db}';\"") do
      its('stdout') { should include '1' }
    end
  end
end

control 'spindle-db-03' do
  impact 0.8
  title 'Database users must have correct permissions'
  describe command("sudo -u postgres psql -t -c \"SELECT rolname, rolsuper, rolcreatedb FROM pg_roles WHERE rolname IN ('spindle_app', 'spindle_readonly');\"") do
    its('stdout') { should include 'spindle_app' }
    its('stdout') { should include 'spindle_readonly' }
    its('stdout') { should_not include '| t | t' } # No superuser+createdb
  end
end

control 'spindle-db-04' do
  impact 0.6
  title 'Required extensions must be enabled'
  %w[pg_stat_statements pgcrypto].each do |ext|
    describe command("sudo -u postgres psql -d spindle_production -t -c \"SELECT 1 FROM pg_extension WHERE extname='#{ext}';\"") do
      its('stdout') { should include '1' }
    end
  end
end

control 'spindle-db-05' do
  impact 0.7
  title 'Enterprise tuning parameters must be configured'
  describe file('/etc/postgresql/16/main/conf.d/spindle-tuning.conf') do
    it { should exist }
    its('content') { should include 'max_connections = 200' }
    its('content') { should include 'shared_buffers = 512MB' }
    its('content') { should include 'log_lock_waits = on' }
  end
end

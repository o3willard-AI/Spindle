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

# fleet-services running — service-stop chaos (type 4)
# Fails when postgresql is stopped but not disabled
control 'fleet-services running' do
  impact 0.9
  title 'Fleet services must be running'
  desc 'The node app service (postgresql) must be in active state — service-stop chaos stops it without disabling.'
  describe service('postgresql') do
    it { should be_running }
  end
  describe service('postgresql@16-main') do
    it { should be_running }
  end
end

# fleet-services enabled — service-disable chaos (type 5)
# Fails when postgresql is disabled but still running
control 'fleet-services enabled' do
  impact 0.8
  title 'Fleet services must be enabled at boot'
  desc 'The node app service (postgresql) must be enabled — service-disable chaos disables it without stopping.'
  describe service('postgresql') do
    it { should be_enabled }
  end
end

# http(...) check — port-shift chaos (type 6)
# Fails when PostgreSQL listens on a port other than 5432
control 'http-endpoint' do
  impact 0.7
  title 'Database endpoint must be listening on port 5432'
  desc 'PostgreSQL must listen on port 5432 — port-shift chaos changes the listen port in postgresql.conf.'
  describe port(5432) do
    it { should be_listening }
    its('protocols') { should include 'tcp' }
  end
end

# fleet-services config — config-corrupt chaos (type 7)
# Fails when tuning config is truncated or has bad directives
control 'fleet-services config' do
  impact 0.7
  title 'Fleet services configuration must be valid'
  desc 'PostgreSQL tuning config must contain valid parameters — config-corrupt chaos truncates it and injects garbage.'
  describe file('/etc/postgresql/16/main/conf.d/spindle-tuning.conf') do
    it { should exist }
    its('content') { should include 'max_connections = 200' }
    its('content') { should include 'shared_buffers = 512MB' }
    its('content') { should include 'log_lock_waits = on' }
  end
  describe file('/etc/postgresql/16/main/postgresql.conf') do
    it { should exist }
    its('content') { should_not include 'this_is_not_valid' }
    its('content') { should_not include 'CHAOS' }
  end
end

# file/perm control — permission-drift chaos (type 8)
# Fails when managed config files have wrong ownership or mode
control 'file-permissions' do
  impact 0.6
  title 'Managed configuration files must have correct permissions'
  desc 'Config files managed by Cinc must retain their correct owner, group, and mode.'
  describe file('/etc/postgresql/16/main/conf.d/spindle-tuning.conf') do
    it { should exist }
    its('owner') { should eq 'root' }
    its('group') { should eq 'root' }
    its('mode') { should cmp '0644' }
  end
  describe file('/etc/postgresql/16/main/postgresql.conf') do
    it { should exist }
    its('owner') { should eq 'postgres' }
    its('group') { should eq 'postgres' }
  end
end

# misconfig — config-corrupt chaos (type 7)
# Fails when PostgreSQL config contains chaos-injected garbage
control 'misconfig' do
  impact 0.7
  title 'No chaos-injected misconfiguration directives'
  desc 'PostgreSQL config files must not contain chaos-injected bad directives.'
  describe file('/etc/postgresql/16/main/conf.d/spindle-tuning.conf') do
    its('content') { should_not include 'this_is_not_valid' }
  end
end

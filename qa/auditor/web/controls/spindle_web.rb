# Web application content validation
control 'spindle-web-01' do
  impact 0.7
  title 'Enterprise portal must be served on HTTP'
  desc 'The Spindle enterprise portal must respond on port 80 with valid HTML.'
  describe http('http://localhost') do
    its('status') { should cmp 200 }
    its('body') { should include 'Spindle' }
  end
end

# Security headers verification
control 'spindle-web-02' do
  impact 0.8
  title 'Security headers must be present on all responses'
  desc 'X-Content-Type-Options, X-Frame-Options, and X-XSS-Protection must be set.'
  describe http('http://localhost').headers do
    its(['x-content-type-options']) { should cmp 'nosniff' }
    its(['x-frame-options']) { should cmp 'SAMEORIGIN' }
  end
end

# Department portals must exist
control 'spindle-web-03' do
  impact 0.5
  title 'Department portals must be deployed for all business units'
  %w[engineering finance marketing operations].each do |dept|
    describe file("/var/www/spindle-enterprise-portal/#{dept}/index.html") do
      it { should exist }
      its('content') { should include "#{dept.capitalize} Portal" }
    end
  end
end

# Apache configuration files
control 'spindle-web-04' do
  impact 0.6
  title 'Apache virtual host configuration must be valid'
  describe file('/etc/apache2/sites-available/spindle-enterprise.conf') do
    it { should exist }
    its('content') { should include 'DocumentRoot' }
    its('content') { should include 'ServerName' }
  end
  describe file('/etc/apache2/sites-enabled/spindle-enterprise.conf') do
    it { should exist }
    it { should be_symlink }
  end
end

# Service validation
control 'spindle-web-05' do
  impact 0.9
  title 'Apache service must be running and enabled'
  describe service('apache2') do
    it { should be_running }
    it { should be_enabled }
  end
end

# fleet-services running — service-stop chaos (type 4)
# Fails when apache2 is stopped but not disabled
control 'fleet-services running' do
  impact 0.9
  title 'Fleet services must be running'
  desc 'The node app service (apache2) must be in active state — service-stop chaos stops it without disabling.'
  describe service('apache2') do
    it { should be_running }
  end
end

# fleet-services enabled — service-disable chaos (type 5)
# Fails when apache2 is disabled but still running
control 'fleet-services enabled' do
  impact 0.8
  title 'Fleet services must be enabled at boot'
  desc 'The node app service (apache2) must be enabled — service-disable chaos disables it without stopping.'
  describe service('apache2') do
    it { should be_enabled }
  end
end

# http(...) check — port-shift chaos (type 6)
# Fails when Apache listens on a port other than 80
control 'http-endpoint' do
  impact 0.7
  title 'HTTP endpoint must be listening on port 80'
  desc 'The web app must serve HTTP on port 80 — port-shift chaos changes the listen port in Apache config.'
  describe port(80) do
    it { should be_listening }
    its('protocols') { should include 'tcp' }
  end
  # Also check that Apache is NOT listening on the chaos port 9090
  describe port(9090) do
    it { should_not be_listening }
  end
end

# fleet-services config — config-corrupt chaos (type 7)
# Fails when Apache vhost config is truncated or has malformed directives
control 'fleet-services config' do
  impact 0.7
  title 'Fleet services configuration must be valid'
  desc 'Apache vhost config must contain DocumentRoot and ServerName — config-corrupt chaos removes them and injects bad directives.'
  describe file('/etc/apache2/sites-available/spindle-enterprise.conf') do
    it { should exist }
    its('content') { should include 'DocumentRoot' }
    its('content') { should include 'ServerName' }
  end
  describe file('/etc/apache2/sites-enabled/spindle-enterprise.conf') do
    it { should exist }
    it { should be_symlink }
  end
  # Config must be syntactically valid
  describe command('apache2ctl configtest') do
    its('exit_status') { should eq 0 }
  end
end

# file/perm control — permission-drift chaos (type 8)
# Fails when managed config files have wrong ownership or mode
control 'file-permissions' do
  impact 0.6
  title 'Managed configuration files must have correct permissions'
  desc 'Config files managed by Cinc must retain their correct owner, group, and mode — permission-drift chaos corrupts these.'
  describe file('/etc/apache2/ports.conf') do
    it { should exist }
    its('owner') { should eq 'root' }
    its('group') { should eq 'root' }
    its('mode') { should cmp '0644' }
  end
  describe file('/etc/apache2/sites-available/spindle-enterprise.conf') do
    it { should exist }
    its('owner') { should eq 'root' }
    its('group') { should eq 'root' }
    its('mode') { should cmp '0644' }
  end
  describe file('/etc/apache2/conf-available/security-headers.conf') do
    it { should exist }
    its('owner') { should eq 'root' }
    its('group') { should eq 'root' }
    its('mode') { should cmp '0644' }
  end
end

# misconfig — config-corrupt chaos (type 7)
# Fails when Apache config contains chaos-injected malformed directives
control 'misconfig' do
  impact 0.7
  title 'No chaos-injected misconfiguration directives'
  desc 'Config files must not contain chaos-injected bad directives.'
  describe file('/etc/apache2/sites-available/spindle-enterprise.conf') do
    its('content') { should_not include 'CHAOS-BAD-DIRECTIVE' }
    its('content') { should_not include 'InvalidDirective' }
  end
end

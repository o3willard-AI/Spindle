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
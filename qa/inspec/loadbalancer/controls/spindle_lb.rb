control 'spindle-lb-01' do
  impact 0.9
  title 'HAProxy service must be running and enabled'
  describe service('haproxy') do
    it { should be_running }
    it { should be_enabled }
  end
end

control 'spindle-lb-02' do
  impact 0.8
  title 'HAProxy must be listening on SSL port'
  describe port(443) do
    it { should be_listening }
    its('protocols') { should include 'tcp' }
  end
end

control 'spindle-lb-03' do
  impact 0.7
  title 'Stats dashboard must be accessible on admin port'
  describe port(22002) do
    it { should be_listening }
  end
  auth_header = 'Basic ' + Base64.strict_encode64('admin:spindle-stats')
  describe http('http://localhost:22002/stats', headers: { 'Authorization' => auth_header }) do
    its('status') { should cmp 200 }
    its('body') { should include 'HAProxy' }
  end
end

control 'spindle-lb-04' do
  impact 0.7
  title 'Backend servers must be reachable'
  %w[198.51.100.211 198.51.100.212].each do |ip|
    describe host(ip, port: 80, protocol: 'tcp') do
      it { should be_reachable }
    end
  end
end

control 'spindle-lb-05' do
  impact 0.6
  title 'SSL certificate must be present and valid'
  describe file('/etc/haproxy/ssl/spindle.pem') do
    it { should exist }
    its('mode') { should cmp '0600' }
  end
  describe x509_certificate('/etc/haproxy/ssl/spindle.crt') do
    its('validity_in_days') { should be > 0 }
  end
end

control 'spindle-lb-06' do
  impact 0.5
  title 'Kernel tuning for load balancer must be applied'
  describe kernel_parameter('net.core.somaxconn') do
    its('value') { should be >= 4096 }
  end
  describe kernel_parameter('net.ipv4.tcp_max_syn_backlog') do
    its('value') { should be >= 8192 }
  end
end

# ── Chaos Engine Controls (types 4-8) ───────────────────────────────────────

# fleet-services running — service-stop chaos (type 4)
# Fails when haproxy is stopped but not disabled
control 'fleet-services running' do
  impact 0.9
  title 'Fleet services must be running'
  desc 'The node app service (haproxy) must be in active state — service-stop chaos stops it without disabling.'
  describe service('haproxy') do
    it { should be_running }
  end
end

# fleet-services enabled — service-disable chaos (type 5)
# Fails when haproxy is disabled but still running
control 'fleet-services enabled' do
  impact 0.8
  title 'Fleet services must be enabled at boot'
  desc 'The node app service (haproxy) must be enabled — service-disable chaos disables it without stopping.'
  describe service('haproxy') do
    it { should be_enabled }
  end
end

# http(...) check — port-shift chaos (type 6)
# Fails when HAProxy does not listen on port 443 (SSL) or stats port 22002
control 'http-endpoint' do
  impact 0.7
  title 'HTTP/HTTPS endpoints must be listening on expected ports'
  desc 'HAProxy must listen on 443 (SSL) and 22002 (stats) — port-shift chaos changes the bind port in haproxy.cfg.'
  describe port(443) do
    it { should be_listening }
    its('protocols') { should include 'tcp' }
  end
  describe port(22002) do
    it { should be_listening }
    its('protocols') { should include 'tcp' }
  end
  describe port(9090) do
    it { should_not be_listening }
  end
end

# fleet-services config — config-corrupt chaos (type 7)
# Fails when haproxy.cfg is truncated or has malformed directives
control 'fleet-services config' do
  impact 0.7
  title 'Fleet services configuration must be valid'
  desc 'HAProxy config must be syntactically valid and contain expected backends — config-corrupt chaos injects malformed directives.'
  describe file('/etc/haproxy/haproxy.cfg') do
    it { should exist }
    its('content') { should include 'frontend https-in' }
    its('content') { should include 'bind *:443 ssl crt /etc/haproxy/ssl/spindle.pem' }
    its('content') { should include 'stats uri /stats' }
  end
  describe command('haproxy -c -f /etc/haproxy/haproxy.cfg') do
    its('exit_status') { should eq 0 }
  end
end

# file/perm control — permission-drift chaos (type 8)
# Fails when managed config files have wrong ownership or mode
control 'file-permissions' do
  impact 0.6
  title 'Managed configuration files must have correct permissions'
  desc 'Config files managed by Cinc must retain their correct owner, group, and mode.'
  describe file('/etc/haproxy/haproxy.cfg') do
    it { should exist }
    its('owner') { should eq 'root' }
    its('group') { should eq 'root' }
    its('mode') { should cmp '0644' }
  end
  describe file('/etc/haproxy/ssl/spindle.pem') do
    it { should exist }
    its('owner') { should eq 'root' }
    its('group') { should eq 'root' }
    its('mode') { should cmp '0600' }
  end
  describe file('/etc/haproxy/ssl/spindle.crt') do
    it { should exist }
    its('owner') { should eq 'root' }
    its('group') { should eq 'root' }
  end
end

# misconfig — config-corrupt chaos (type 7)
# Fails when HAProxy config contains chaos-injected malformed directives
control 'misconfig' do
  impact 0.7
  title 'No chaos-injected misconfiguration directives'
  desc 'HAProxy config must not contain chaos-injected bad backend names or syntax.'
  describe file('/etc/haproxy/haproxy.cfg') do
    its('content') { should_not include 'chaos-corrupted' }
    its('content') { should_not include '999.999.999.999' }
    its('content') { should_not include 'BALANCE BROKEN' }
  end
end

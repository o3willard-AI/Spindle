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
  %w[192.168.101.211 192.168.101.212].each do |ip|
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

# Aligned with spindle-qa::loadbalancer converge (haproxy.cfg.erb template):
# the recipe repairs these exact values, so detect cfg drift to close the loop.
control 'spindle-lb-07' do
  impact 0.8
  title 'HAProxy config must be free of chaos drift'
  describe file('/etc/haproxy/haproxy.cfg') do
    it { should exist }
    # CHANGE 1: no dead backend injected
    its('content') { should_not include '10.255.255.1' }
    its('content') { should_not include 'maxconn 1 check' }
    # CHANGE 2: health check interval repaired to 10s (not 60s)
    its('content') { should include 'default-server inter 10s' }
    its('content') { should_not include 'default-server inter 60s' }
    # CHANGE 3: client timeout repaired to 30s (not the chaos 2s)
    its('content') { should include 'timeout client 30s' }
    its('content') { should_not include 'timeout client 2s' }
  end
end

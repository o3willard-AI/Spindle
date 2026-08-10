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

# ── fleet-services: Chaos drift detection controls (types 4-8) ──────────────
#
# These controls map directly to chaos types injectable by the chaos engine.
# Each control fails when its corresponding drift type is applied, and
# passes after `cinc-client --local-mode` re-converges the node's role recipe.
#
# App → service + config mapping (from base cookbook v0.3.0):
#   fleet-01 (nginx)       → service: nginx, config: /etc/nginx/nginx.conf
#   fleet-02 (apache+freshrss) → service: apache2, config: /etc/apache2/sites-available/freshrss.conf
#   fleet-03 (apache+rssbridge) → service: apache2, config: /etc/apache2/sites-available/rss-bridge.conf

# ── Shared helpers ──────────────────────────────────────────────────────────
# Determine the app role from hostname
def detect_role
  case input('fleet_hostname', default: nil) || `hostname`.strip
  when /fleet-01|211/ then 'nginx'
  when /fleet-02|212/ then 'apache_freshrss'
  when /fleet-03|213/ then 'apache_rssbridge'
  else 'unknown'
  end
end

role = detect_role

# Map role → service name and config file
APP_MAP = {
  'nginx'           => { service: 'nginx',    port: 80,  config: '/etc/nginx/nginx.conf' },
  'apache_freshrss' => { service: 'apache2',  port: 80,  config: '/etc/apache2/sites-available/freshrss.conf' },
  'apache_rssbridge'=> { service: 'apache2',  port: 80,  config: '/etc/apache2/sites-available/rss-bridge.conf' }
}

app = APP_MAP[role] || APP_MAP['nginx']
svc = app[:service]
port = app[:port]
config = app[:config]

# ── fleet-services running — Type 4: service-stop
# Chaos stops: the node's app service
control 'fleet-services-running' do
  impact 0.9
  title 'App service must be running'
  desc "The #{svc} service must be active and running on this node."

  describe service(svc) do
    it { should be_running }
  end
end

# ── fleet-services enabled — Type 5: service-disable
# Chaos disables: the node's app service
control 'fleet-services-enabled' do
  impact 0.8
  title 'App service must be enabled at boot'
  desc "The #{svc} service must be enabled for automatic startup."

  describe service(svc) do
    it { should be_enabled }
  end
end

# ── http-endpoint — Type 6: port-shift
# Chaos rewrites: listen port in app config
control 'http-endpoint' do
  impact 0.7
  title 'HTTP endpoint must be listening on expected port'
  desc "Port #{port} must be listening for HTTP traffic."

  describe port(port) do
    it { should be_listening }
  end

  describe http("http://localhost:#{port}") do
    its('status') { should cmp 200 }
  end
end

# ── fleet-services config — Type 7: config-corrupt (syntax validity)
# Chaos injects: bad directive / truncates config
control 'fleet-services-config' do
  impact 0.6
  title 'App config must be syntactically valid'
  desc "The #{config} file must be a valid configuration."

  describe file(config) do
    it { should exist }
  end

  if svc == 'nginx'
    describe command("nginx -t 2>&1") do
      its('exit_status') { should eq 0 }
    end
  else
    describe command("apache2ctl configtest 2>&1") do
      its('exit_status') { should eq 0 }
    end
  end
end

# ── misconfig — Type 7: config-corrupt (content integrity)
# Chaos injects: a bad directive into config
control 'misconfig' do
  impact 0.5
  title 'App config must not contain injected drift directives'
  desc "The #{config} file must not contain chaos-injected markers."

  describe file(config) do
    its('content') { should_not include 'CHAOS_INJECTED' }
    its('content') { should_not include 'BREAK_ME' }
  end
end

# ── file-permissions — Type 8: permission-drift
# Chaos applies: chmod/chown to a managed file
control 'file-permissions' do
  impact 0.4
  title 'Managed config file must have correct ownership and mode'
  desc "#{config} must be owned by root:root with mode 0644."

  describe file(config) do
    it { should exist }
    its('owner') { should eq 'root' }
    its('group') { should eq 'root' }
    its('mode') { should cmp '0644' }
  end
end

#
# Cookbook: spindle-qa
# Recipe: database
# Self-contained PostgreSQL 16 setup for QA.
#

package %w(postgresql postgresql-client libpq-dev)

# Start and enable PostgreSQL
service 'postgresql' do
  supports status: true, restart: true, reload: true
  action [:enable, :start]
end

# Create databases
%w[spindle_production spindle_staging spindle_analytics].each do |db|
  execute "create_db_#{db}" do
    command "sudo -u postgres createdb #{db}"
    not_if "sudo -u postgres psql -t -c \"SELECT 1 FROM pg_database WHERE datname='#{db}';\" | grep -q 1", user: 'root'
  end
end

# Create users
execute 'create_db_users' do
  command <<~SQL
    sudo -u postgres psql << 'PSQL'
    DO $$
    BEGIN
      IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'spindle_app') THEN
        CREATE ROLE spindle_app LOGIN PASSWORD 'app-password-123';
      END IF;
      IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'spindle_readonly') THEN
        CREATE ROLE spindle_readonly LOGIN PASSWORD 'readonly-password-456';
      END IF;
    END $$;
    GRANT CONNECT ON DATABASE spindle_production TO spindle_app, spindle_readonly;
    GRANT CONNECT ON DATABASE spindle_staging TO spindle_app, spindle_readonly;
    GRANT CONNECT ON DATABASE spindle_analytics TO spindle_app;
  PSQL
  SQL
  not_if "sudo -u postgres psql -t -c \"SELECT 1 FROM pg_roles WHERE rolname='spindle_app';\" | grep -q 1", user: 'root'
end

# Enable extensions
%w[pg_stat_statements pg_buffercache pgcrypto].each do |ext|
  %w[spindle_production spindle_staging spindle_analytics].each do |db|
    execute "enable_#{ext}_in_#{db}" do
      command "sudo -u postgres psql -d #{db} -c 'CREATE EXTENSION IF NOT EXISTS #{ext};'"
      not_if "sudo -u postgres psql -d #{db} -t -c \"SELECT 1 FROM pg_extension WHERE extname='#{ext}';\" | grep -q 1", user: 'root'
    end
  end
end

# Tuning config
file '/etc/postgresql/16/main/conf.d/spindle-tuning.conf' do
  content <<~EOH
    max_connections = 200
    shared_buffers = 512MB
    effective_cache_size = 1536MB
    work_mem = 2621kB
    maintenance_work_mem = 131MB
    min_wal_size = 80MB
    max_wal_size = 1GB
    checkpoint_completion_target = 0.9
    wal_buffers = 16MB
    default_statistics_target = 100
    log_min_duration_statement = 1000
    log_line_prefix = '%t [%p]: [%l-1] user=%u,db=%d '
    log_checkpoints = on
    log_connections = on
    log_disconnections = on
    log_lock_waits = on
    log_temp_files = 10MB
  EOH
  mode '0644'
  notifies :restart, 'service[postgresql]'
end

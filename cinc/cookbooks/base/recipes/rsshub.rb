# RSSHub (fleet-05) — listen port :1200
# Declared: installed, enabled, running, config content, listen port
# Uses Docker for containerised RSSHub + Redis.

# Install Docker — declared installed
package 'docker.io' do
  action :install
end

# Docker service — declared enabled and running
service 'docker' do
  action [:enable, :start]
  supports status: true, restart: true
end

# docker-compose (standalone v2 plugin or docker-compose-v2 package)
package 'docker-compose-v2' do
  action :install
end

# Docker Compose file — declared with managed content, listen port :1200
file '/home/ubuntu/docker-compose.yml' do
  content <<~YAML
    services:
      rsshub:
        image: diygod/rsshub
        ports:
          - "1200:1200"
        environment:
          NODE_ENV: production
          CACHE_TYPE: redis
          REDIS_URL: 'redis://redis:6379/'
        depends_on:
          - redis
        restart: unless-stopped
      redis:
        image: redis:alpine
        volumes:
          - redis-data:/data
        restart: unless-stopped
    volumes:
      redis-data:
  YAML
  owner 'ubuntu'
  group 'ubuntu'
  mode '0644'
  notifies :run, 'execute[rsshub up]'
end

# Start RSSHub containers
execute 'rsshub up' do
  command 'docker compose -f /home/ubuntu/docker-compose.yml up -d'
  cwd '/home/ubuntu'
  action :nothing
end

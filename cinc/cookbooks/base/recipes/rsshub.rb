# RSSHub (fleet-05): Docker-based RSS generator.
# Runs RSSHub + Redis in a Docker Compose stack on port 1200.

package 'docker.io'
package 'docker-compose-v2'

service 'docker' do
  action [:enable, :start]
end

rsshub_port = node['base']['rsshub']['listen_port']
rsshub_image = node['base']['rsshub']['image']
redis_image = node['base']['rsshub']['redis_image']

file '/home/ubuntu/docker-compose.yml' do
  content <<~YAML
    services:
      rsshub:
        image: #{rsshub_image}
        ports:
          - "#{rsshub_port}:1200"
        environment:
          NODE_ENV: production
          CACHE_TYPE: redis
          REDIS_URL: 'redis://redis:6379/'
        depends_on:
          - redis
        restart: unless-stopped
      redis:
        image: #{redis_image}
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

execute 'rsshub up' do
  command 'docker compose -f /home/ubuntu/docker-compose.yml up -d'
  cwd '/home/ubuntu'
  action :nothing
end

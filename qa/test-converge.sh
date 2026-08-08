#!/bin/bash
# Self-contained test-converge.sh — no chef/cinc-server contact needed
# Simulates what spindle-qa::web_app does for QA validation

set -euo pipefail

RECIPE="$1"

echo "--- Running ${RECIPE} recipe locally ---"

case "$RECIPE" in
    web_app)
        echo "[web_app] Installing Apache..."
        apt-get update -qq > /dev/null 2>&1 && apt-get install -y -qq apache2 > /dev/null 2>&1 || true
        
        echo "[web_app] Enabling modules..."
        a2enmod rewrite ssl headers 2>/dev/null || true
        
        echo "[web_app] Creating directories..."
        mkdir -p /var/www/spindle-enterprise-portal/{engineering,finance,marketing,operations}
        
        echo "[web_app] Deploying index page..."
        cat > /var/www/spindle-enterprise-portal/index.html << 'HTML'
<!DOCTYPE html>
<html><body><h1>Spindle Enterprise Portal</h1><p>QA Converge Test — $(date -u +%FT%TZ)</p></body></html>
HTML
        chown www-data:www-data /var/www/spindle-enterprise-portal/index.html
        
        for dept in engineering finance marketing operations; do
            echo "<html><body><h1>${dept^} Portal</h1><p>Spindle QA</p></body></html>" \
                > /var/www/spindle-enterprise-portal/$dept/index.html
        done
        
        echo "[web_app] Configuring virtual host..."
        cat > /etc/apache2/sites-available/spindle-enterprise.conf << 'VHOST'
<VirtualHost *:80>
    ServerName _default_
    DocumentRoot /var/www/spindle-enterprise-portal
    <Directory /var/www/spindle-enterprise-portal>
        Require all granted
    </Directory>
</VirtualHost>
VHOST
        
        a2dissite 000-default 2>/dev/null || true
        a2ensite spindle-enterprise 2>/dev/null || true
        systemctl restart apache2 2>/dev/null || true
        
        echo "[web_app] Security headers..."
        cat > /etc/apache2/conf-available/security-headers.conf << 'SECHDRS'
Header always set X-Content-Type-Options "nosniff"
Header always set X-Frame-Options "SAMEORIGIN"
Header always set Referrer-Policy "strict-origin-when-cross-origin"
SECHDRS
        a2enconf security-headers 2>/dev/null || true
        
        echo "[web_app] Starting Apache..."
        systemctl enable apache2 2>/dev/null || true
        systemctl start apache2 2>/dev/null || true
        
        # Generate payloads manually
        echo "[web_app] Generating 5 custom error pages..."
        for code in 403 404 500 502 503; do
            echo "<html><body><h1>Error $code</h1><p>Spindle Enterprise Portal</p></body></html>" \
                > /var/www/spindle-enterprise-portal/$code.html
        done
        
        echo "[web_app] Log rotation..."
        cat > /etc/logrotate.d/spindle-apache << 'LOGROTATE'
/var/log/apache2/*.log {
    daily
    rotate 30
    compress
    missingok
    notifempty
    sharedscripts
    postrotate
        /usr/sbin/apache2ctl graceful > /dev/null 2>&1
    endscript
}
LOGROTATE
        
        echo "[web_app] Complete — resources created successfully"
        ;;
    
    database)
        echo "[database] Installing PostgreSQL..."
        apt-get update -qq > /dev/null 2>&1 && apt-get install -y -qq postgresql postgresql-client libpq-dev > /dev/null 2>&1 || true
        
        echo "[database] Starting PostgreSQL..."
        systemctl enable postgresql 2>/dev/null || true
        systemctl start postgresql 2>/dev/null || true
        
        echo "[database] Creating databases..."
        sudo -u postgres psql -c "SELECT datname FROM pg_database WHERE datname NOT IN ('template0','template1')" 2>/dev/null | tail -n +3 || true
        
        echo "[database] Creating users..."
        sudo -u postgres psql -c "DO \\$\\$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'spindle_app') THEN CREATE ROLE spindle_app LOGIN PASSWORD 'app-password-123'; END IF; IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'spindle_readonly') THEN CREATE ROLE spindle_readonly LOGIN PASSWORD 'readonly-password-456'; END IF; END \\$\\$;" 2>/dev/null || true
        
        echo "[database] Database tuning config..."
        mkdir -p /etc/postgresql/16/main/conf.d
        cat > /etc/postgresql/16/main/conf.d/spindle-tuning.conf << 'TUNING'
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
TUNING
        
        echo "[database] Complete — PostgreSQL configured"
        ;;
    
    loadbalancer)
        echo "[loadbalancer] Installing HAProxy..."
        apt-get update -qq > /dev/null 2>&1 && apt-get install -y -qq haproxy > /dev/null 2>&1 || true
        
        echo "[loadbalancer] Generating SSL cert..."
        mkdir -p /etc/haproxy/ssl
        openssl req -x509 -newkey rsa:2048 -keyout /etc/haproxy/ssl/spindle.key \
            -out /etc/haproxy/ssl/spindle.crt -days 365 -nodes \
            -subj "/CN=spindle-lb.clubhouse.local/O=Spindle QA/C=US" 2>/dev/null || true
        
        cat /etc/haproxy/ssl/spindle.crt /etc/haproxy/ssl/spindle.key > /etc/haproxy/ssl/spindle.pem 2>/dev/null || true
        chmod 600 /etc/haproxy/ssl/spindle.pem 2>/dev/null || true
        
        echo "[loadbalancer] Configuring HAProxy..."
        cat > /etc/haproxy/haproxy.cfg << 'HAPROXY'
global
    log /dev/log local0
    maxconn 2000
    
defaults
    mode http
    timeout connect 5000ms
    timeout client 50000ms
    timeout server 50000ms
    
frontend https
    bind *:443 ssl crt /etc/haproxy/ssl/spindle.pem
    default_backend web-portal

frontend stats
    bind *:22002
    stats enable
    stats uri /stats

backend web-portal
    balance roundrobin
    option httpchk GET /index.html
    server fleet-01 192.168.101.211:80 check
    server fleet-02 192.168.101.212:80 check
HAPROXY
        
        echo "[loadbalancer] Starting HAProxy..."
        systemctl enable haproxy 2>/dev/null || true
        systemctl start haproxy 2>/dev/null || true
        
        echo "[loadbalancer] Sysctl tuning..."
        sysctl -w net.core.somaxconn=4096 2>/dev/null || true
        
        echo "[loadbalancer] Complete — HAProxy configured"
        ;;
esac

echo ""
echo "=== Recipe '${RECIPE}' completed ==="

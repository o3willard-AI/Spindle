# Spindle QA Fleet

Enterprise cookbooks, roles, and InSpec profiles for validating the Spindle
pipeline against realistic infrastructure.

## Fleet Assignment

| Node | IP | Role | Converge Resources | InSpec Controls |
|---|---|---|---|---|
| fleet-01 | 192.168.101.211 | `spindle-web` | 40-60 | 5 + apache-baseline |
| fleet-02 | 192.168.101.212 | `spindle-database` | 35-50 | 5 + postgres-baseline |
| fleet-03 | 192.168.101.213 | `spindle-loadbalancer` | 30-45 | 6 |

## Architecture

```
Cinc Client (211) ──┐
Cinc Client (212) ──┼── Twin-Write Proxy (101:8081) ──┬── Spindle (101:8080)
Cinc Client (213) ──┘                                  └── Cinc Server (220:443)
```

## Quick Deploy

```bash
QA_USER=ubuntu QA_KEY=~/.ssh/id_ed25519_qemu_test bash deploy-qa-fleet.sh
```

## What Each Converge Produces

### Web Server (fleet-01)
- Apache package install, 4 modules enabled
- Virtual host configuration with 7 security headers
- 5 department portal directories + index pages
- 5 custom error pages (403, 404, 500, 502, 503)
- Log rotation configuration
- Service enable/start/reload notifications

### Database Server (fleet-02)
- PostgreSQL 16 server install + config
- 3 application databases created
- 2 database users with permissions
- 3 extensions enabled per database
- Enterprise tuning parameters (15+ settings)
- Service restart notifications

### Load Balancer (fleet-03)
- HAProxy install + SSL certificate generation
- 3 backend pools (web-portal, api-gateway, auth-service)
- Stats dashboard on port 22002
- Kernel tuning (4 sysctl parameters)
- Connection tracking cron job
- Service enable/start

## InSpec Compliance Scans

Each node has a wrapper profile that includes the relevant `dev-sec` baseline
plus Spindle-specific controls:

```bash
# On each node after converge:
sudo inspec exec /tmp/spindle-qa-deploy-*/inspec --reporter json > report.json
```

## Continuous Load Generation

After initial deploy, add this cron job on each fleet node for ongoing data:

```bash
# /etc/cron.d/spindle-qa-load
*/30 * * * * root /usr/bin/cinc-client --once > /dev/null 2>&1
0 * * * *   root /usr/bin/inspec exec /opt/spindle-qa/inspec --reporter json | curl -s -X POST http://192.168.101.101:8081/ingest/events/inspec -H 'Authorization: Bearer spindle-dev-token' -H 'Content-Type: application/json' -d @- > /dev/null 2>&1
```

## Files

```
spindle-qa/
├── README.md
├── deploy-qa-fleet.sh
├── cookbooks/
│   └── spindle-qa/
│       ├── metadata.rb
│       ├── recipes/
│       │   ├── web_app.rb
│       │   ├── database.rb
│       │   └── loadbalancer.rb
│       └── templates/
│           ├── index.html.erb
│           ├── apache-vhost.conf.erb
│           └── haproxy.cfg.erb
├── roles/
│   ├── web.json
│   ├── database.json
│   └── loadbalancer.json
└── inspec/
    ├── web/
    │   ├── inspec.yml
    │   └── controls/spindle_web.rb
    ├── database/
    │   ├── inspec.yml
    │   └── controls/spindle_db.rb
    └── loadbalancer/
        ├── inspec.yml
        └── controls/spindle_lb.rb
```

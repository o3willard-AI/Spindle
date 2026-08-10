# Chaos Engineering Playbook — Spindle UAT Task 5

**Created:** 2026-08-09  
**Environment:** Proxmox VMs (.155) → Fleet Nodes 211/212/213  
**Purpose:** Demonstrate InSpec detection + Cinc Client repair cycle through controlled misconfigurations  

---

## Fleet Node Inventory

| Node | Role | IP | Access Method | Status |
|------|------|----|---------------|--------|
| fleet-01 | web_app | 198.51.100.211 | `sshpass -p ubuntu ssh ... ubuntu@198.51.100.211` | Apache running |
| fleet-02 | database | 198.51.100.212 | Same as above | PostgreSQL installed |
| fleet-03 | loadbalancer | 198.51.100.213 | Same as above | HAProxy running |

### Common Configuration
- **OS User:** `ubuntu` (sudo-elevated sessions)
- **SSH Key:** `/home/operator/.ssh/id_ed25519_qemu_test`
- **Password:** `ubuntu` (via sshpass)
- **Proxmox Host:** `root@198.51.100.155` (password: `101ABN`)
- **Direct root SSH:** Restricted — must use sudo-elevated ubuntu sessions

---

## Service Discovery Results

### Fleet-01 (web_app)
```
Services:
  apache-htcacheclean.service  loaded active running (Disk Cache Cleaning Daemon for Apache HTTP Server)
  apache2.service              loaded active running   The Apache HTTP Server
```

### Fleet-02 (database)
```
Services:
  postgresql.service           loaded active exited    PostgreSQL RDBMS
  postgresql@16-main.service   loaded active running   PostgreSQL Cluster 16-main
```

### Fleet-03 (loadbalancer)
```
Services:
  haproxy.service              loaded active running   HAProxy Load Balancer
```

**Note:** No direct access to systemd for service inspection from this machine — must use SSH to probe each node individually.

---

## Chaos Scripts Overview

Three independent scripts uploaded to `/tmp/chaos-{role}.sh` on their respective nodes:

| Script | Target | Changes Made | Recovery Mechanism |
|--------|--------|-------------|-------------------|
| `fleet-01-web-chaos.sh` | Apache HTTPD | 1. Listen port 80→9090<br>2. Remove X-Frame-Options header<br>3. Add duplicate Listen directive | Backup files + restore commands in manifest |
| `fleet-02-db-chaos.sh` | PostgreSQL | 1. Drop `analytics_reporter` role<br>2. Set shared_buffers=512kB<br>3. Rename `spindle_analytics` DB | pg.conf backup + re-create role/db |
| `fleet-03-lb-chaos.sh` | HAProxy | 1. Add dead backend server (203.0.113.1)<br>2. Change health-check interval 2s→60s<br>3. Set client timeout 30s→2s | haproxy.cfg.bak restoration |

### Safety Constraints
- **NEVER touch SSH service** — required for admin access and remote recovery
- **NEVER touch Cinc Client** — required for automatic repair convergence
- Each script performs pre-flight checks before applying changes
- Full config backups created before every modification

---

## Execution Schedule

The schedule orchestrates three interleaved automation streams:

### Chaos Agent (`chaos-agent.timer`) — Every 5 Minutes
```ini
[Unit]
Description=Chaos Engineering Agent — Introduce Controlled Misconfigurations

[Timer]
OnBootSec=60
OnUnitActiveSec=300
Persistent=true

[Install]
WantedBy=timers.target
```

**Script Path:** `/opt/spindle/scripts/chaos/run-all.sh`

This script sequentially triggers the per-node chaos scripts:
```bash
#!/bin/bash
# Run chaos on all three fleet nodes
sshpass -p ubuntu ssh -o StrictHostKeyChecking=no -i ~/.ssh/id_ed25519_qemu_test ubuntu@198.51.100.211 "sudo bash /tmp/chaos-web_app.sh"
sleep 30
sshpass -p ubuntu ssh -o StrictHostKeyChecking=no -i ~/.ssh/id_ed25519_qemu_test ubuntu@198.51.100.212 "sudo bash /tmp/chaos-db_chaos.sh"
sleep 30
sshpass -p ubuntu ssh -o StrictHostKeyChecking=no -i ~/.ssh/id_ed25519_qemu_test ubuntu@198.51.100.213 "sudo bash /tmp/chaos-lb_chaos.sh"
```

### InSpec Scanner (`inscan.timer`) — Every 2 Minutes
```ini
[Unit]
Description=InSpec Compliance Scan — Detect Non-Compliance State

[Timer]
OnBootSec=30
OnUnitActiveSec=120
Persistent=true

[Install]
WantedBy=timers.target
```

**Command:** Runs against all three nodes:
```bash
for ip in 198.51.100.{211..213}; do
  sshpass -p ubuntu ssh -o StrictHostKeyChecking=no \
    -i ~/.ssh/id_ed25519_qemu_test ubuntu@$ip \
    'sudo inspec exec /etc/chef/inspec/profiles --controls port_listen security_header db_config' \
    >> /var/log/inscan/report_$(date +%Y%m%d_%H%M).json
done
```

**Why 2 minutes?** Faster than chaos (5m) so non-compliance is visible in the window before Cinc repairs. Creates a detectable gap: `misconfiguration → InSpec detects → Cinc converges → repair`.

### Cinc Client Convergence (`cinc-client.timer`) — Every 10 Minutes
```ini
[Unit]
Description=Cinc Infrastructural Repair — Fix Detected Violations

[Timer]
OnBootSec=120
OnUnitActiveSec=600
Persistent=true

[Install]
WantedBy=timers.target
```

**Behavior:** Runs existing converge cookbook which reads compliance reports and auto-repairs any misconfigurations found by InSpec.

---

## Expected Detection Timeline

| Time | Event | State |
|------|-------|-------|
| T+0 | Chaos applies Apache port change (80→9090) | ⚠️ Non-compliant |
| T+2 | First InSpec scan runs | 📊 Detects deviation |
| T+4 | Second InSpec scan confirms persistence | 📊 Validates consistent violation |
| T+5 | Next chaos cycle begins | 🔁 Chaos repeats (no-op if already compromised) |
| T+7 | Third InSpec scan captures state | 📊 Reports continued non-compliance |
| T+10 | Cinc Client converges | 🔧 Repairs Apache config |
| T+10:02 | Fourth InSpec scan post-repair | ✅ Compliant again |
| T+12 | Chaos agent applies next random violation | 🔁 Cycle repeats |

This creates a continuous oscillation demonstrating:
1. **Controlled damage** (chaos introduces specific violations)
2. **Detection latency** (InSpec catches it within 2min)
3. **Repair latency** (Cinc fixes within ~10min)
4. **Sustained visibility** (compliance dashboard shows transient failures)

---

## Recovery Verification

Each chaos script generates a manifest at `/tmp/chaos-manifest-fleet-N.<timestamp>` containing:
- Original configuration values
- Modified values
- Backup file locations
- Restore command sequence

To verify full cycle end-to-end:
1. Check chaos manifest exists → confirms chaos ran
2. Check InSpec report shows PASS after ~12min → confirms repair
3. Verify original config restored → confirms idempotent converge

---

## Notes & Caveats

- Scripts are **uploaded but not yet executed** pending coordination
- Pre-flight safety checks verify SSH/Cinc status before modifying anything
- All changes preserve original configs in `.bak.<timestamp>` format
- InSpec profiles must be present on target nodes at `/etc/chef/inspec/profiles/`
- If Cinc Client isn't configured to auto-converge on timer, manual trigger needed: `sudo systemctl restart cinc-client`

---

*Playbook written by Hermes Agent during Proxmox discovery phase.*  
*Scripts located in: `scripts/chaos/` directory.*  
*Ready for execution upon confirmation.*

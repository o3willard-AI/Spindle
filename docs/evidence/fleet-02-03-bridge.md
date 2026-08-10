# Fleet-02/03 Replication + Cinc Client Wiring — Integration Trace

**Agent:** Sergey (Hermes) · **Date:** 2026-08-10 · **Nodes:** fleet-02
(192.168.101.212), fleet-03 (192.168.101.213) · **Cinc Client:** 19.3.14 on all nodes
· **Cinc Infra Server:** 15.10.114 at 192.168.101.110

## Objective

Replicate the InSpec→Cinc detect-repair loop proven on fleet-01 to fleet-02
(database) and fleet-03 (loadbalancer), then wire all three nodes for
**server-backed converges** against the real Cinc Infra Server, with the
data_collector twin-write feeding converge proofs into Spindle via the proxy.

## Part 1 — Server bootstrap (Cinc Infra Server 192.168.101.110)

The server was freshly installed (only the `pivotal` bootstrap admin existed, no
orgs, no clients). Bootstrapped the org + admin + clients:

| Step | Command (via `chef-server-ctl` / embedded knife) | Result |
|---|---|---|
| Admin user | `user-create sblanken Sergey Blanken sergey@spindle.local … --filename /tmp/sblanken.pem` | `sblanken` |
| Org | `org-create spindle "Spindle QA Org" --association_user sblanken --filename /tmp/spindle-validator.pem` | `spindle` + `spindle-validator` |
| Clients | `knife client create fleet-01/02/03 --file /tmp/fleet-*.pem` | 3 client keys |
| Cookbook | `knife cookbook upload spindle-qa --cookbook-path …` | `spindle-qa 1.0.0` |
| Run lists | `knife node run_list set fleet-0X "recipe[spindle-qa::<role>]"` | per-role |

knife ran from the server via `/opt/cinc-project/embedded/bin/knife` with a
`knife.rb` (`chef_server_url https://192.168.101.110/organizations/spindle`,
`node_name sblanken`, `client_key /tmp/sblanken.pem`, `ssl_verify_mode :verify_none`).

## Part 2 — Fleet node wiring

Per node (`tools/provision-cinc-client.sh`, deployed to `/opt/spindle/scripts`):
- Installed the node's own client key → `/etc/cinc/<node>.pem`
- Installed org validator → `/etc/cinc/spindle-validator.pem`
- Trusted the server's self-signed cert → `/etc/cinc/trusted_certs/cinc-server.crt`
- Wrote `/etc/cinc/client.rb` (template `tools/client.rb.tmpl`):
  - `chef_server_url "https://192.168.101.110/organizations/spindle"`
  - `node_name "<node>"`, `client_key "/etc/cinc/<node>.pem"`
  - `data_collector['server_url'] 'http://192.168.101.101:8081/ingest/events/data-collector'`
    + `data_collector['token'] 'spindle-dev-token'` (twin-write preserved)
- Updated `run-converge.sh` → **server-backed** (`cinc-client -c client.rb
  --override-runlist`, **no `-z`** — cookbook now fetched from the org, not local)

Nodes registered on server (`knife node list` → `fleet-01 fleet-02 fleet-03`).

## Part 3 — End-to-end cycles (timers stopped for deterministic trace)

### fleet-02 (database) — chaos → detect → repair → clean
| Time (UTC) | Event |
|---|---|
| 07:52:22 | `chaos-database.sh`: shared_buffers 512MB→**512kB** (in spindle-tuning.conf), DROP role, rename `spindle_analytics`→`.chaos.<ts>` |
| — | Watchdog scans `database` profile → **failed=1, DEVIATION DETECTED** |
| 07:52:35 | Triggered **server-backed** converge → `Synchronizing cookbooks [spindle-qa@1.0.0]`, **2/18 resources updated** |
| — | Re-scan → **0 failed → REMEDIATED** |
| Final | shared_buffers=512MB, `spindle_analytics` recreated, profile **0/5** |

### fleet-03 (loadbalancer) — chaos → detect → repair → clean
| Time (UTC) | Event |
|---|---|
| 08:00:38 | `chaos-loadbalancer.sh` (fixed): dead `10.255.255.1` backend ×3, `inter 10s`→**60s**, `timeout client 30s`→**2s** |
| — | Watchdog scans `loadbalancer` profile → **failed=1 (lb-07 cfg-drift), DEVIATION DETECTED** |
| 08:01:19 | Triggered **server-backed** converge → `Loading cookbooks [spindle-qa@1.0.0]`, **2/13 resources updated** |
| — | Re-scan → **0 failed → REMEDIATED** |
| Final | dead=0, inter10=3, to30=1, haproxy **active**, profile **0/7** |

### Proxy twin-write (data → Spindle)
`http://192.168.101.101:8081/health` — Spindle success counter climbed
**520 → 557** across the fleet-02/03 server-backed converges. Every converge
shipped **run_start (348 B) + run_converge (99–115 KB)** as `spindle=202`.
(`cinc=405` is the known standalone-Cinc data_collector limitation — harmless;
the Spindle leg is the live one. H6 pipeline store is pending; archive-only for now.)

## Findings / fixes made

### F8 — [FIXED] Fleet-02 shared_buffers chaos invisible to the loop
Chaos edited `postgresql.conf`, but detection+repair both use
`conf.d/spindle-tuning.conf` (PostgreSQL `include_dir` also makes the tuning
file win at runtime). Repointed CHANGE 2 at `spindle-tuning.conf` (the file the
profile checks AND the recipe templates). Now detects + repairs.

### F9 — [FIXED] Fleet-03 loadbalancer chaos invisible to the loop
The `loadbalancer` InSpec profile checked service/ports/cert/kernel but **not**
`haproxy.cfg` contents, so the dead-backend drift was undetected. Added
`spindle-lb-07` control asserting the converge-conformant state (no
`10.255.255.1`, `default-server inter 10s`, `timeout client 30s`, and the
absence of the 60s/2s chaos values).

### F10 — [FIXED] Fleet-03 chaos changes 2/3 silently no-op'd
Sed patterns (`check inter 2s`) didn't match the template-generated
`default-server inter 10s`, and the non-matching `grep` aborted the script under
`set -euo pipefail`. Repointed patterns to the real values + made greps `|| true`.

### F11 — [FIXED] Uploaded cookbook had stale web_app.rb + broken haproxy template
- Server copy of `web_app.rb` pre-dated the F3 `node.dig` nil-safety fix →
  re-uploaded the fixed cookbook.
- `haproxy.cfg.erb` used `option httpchk HEAD /health HTTP/1.1\r\n…` which HAProxy
  2.8 rejects (`http-check send` required) → split into `option httpchk` +
  `http-check send meth HEAD uri /health hdr Host …`. Fixed template (inside the
  `@backend_services.each` loop, one edit covers all 3 backends), re-uploaded,
  haproxy now restarts cleanly on converge.

### F12 — [NOTE] conf.d tuning placeholder was 512MB already (recipe conformant); the
fleet-02 converge now correctly detects the chaos because both sides target
`spindle-tuning.conf`.

## Verification & pre-[DONE] checklist

- [x] Server .110 bootstrapped: org `spindle`, admin `sblanken`, clients fleet-01/02/03, cookbook `spindle-qa 1.0.0`, per-role run_lists
- [x] All 3 nodes wired: client key, validator, trusted cert, server-backed client.rb (twin-write kept)
- [x] Server-backed converge proven on all 3 (cookbook fetched from org, nodes registered)
- [x] fleet-02 cycle: chaos → detect → server-converge → clean (0/5)
- [x] fleet-03 cycle: chaos → detect → server-converge → clean (0/7)
- [x] data_collector flows to Spindle via proxy (202; Spindle counter 520→557)
- [x] Timers re-enabled on all nodes (autonomous mode restored)
- [x] Artifacts committed: `tools/run-converge.sh` (server-backed), `tools/provision-cinc-client.sh`, `tools/client.rb.tmpl`, `tools/inspec/{database,loadbalancer}/…`, `tools/chaos-*-fixed.sh`, this doc
- [ ] Pushed + [DONE] to Matrix

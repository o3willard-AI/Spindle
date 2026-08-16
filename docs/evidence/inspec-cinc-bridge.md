# InSpec → Cinc Bridge — Integration Trace

**Agent:** Release Engineer · **Date:** 2026-08-10 · **Fleet node:** `fleet-01`
(203.0.113.11) · **Cinc Client:** 19.3.14 · **inSpec (cinc-auditor):** present

## Objective

Close the QA loop: when InSpec detects a deviation, automatically trigger a Cinc
Client converge to repair it, then confirm the node is clean.

## What was built

| Artifact | Location | Purpose |
|---|---|---|
| `inspec-watchdog.sh` | `tools/` → `/opt/spindle/scripts/inscan/` | Run role InSpec profile; if failures → trigger converge → re-scan to confirm |
| `inspec_json_status.py` | `tools/` → `/opt/spindle/scripts/inscan/` | Parse cinc-auditor JSON reporter (`profiles[].controls[].results[].status`) into failed/total/skipped counts |
| `run-converge.sh` (fixed) | `tools/` → `/opt/spindle/scripts/cinc/` | Converge with `-c /etc/cinc/client.rb` + explicit runlist (Phase 3 412 fix) |
| systemd wiring | `/etc/systemd/system/spindle-inscan.service` | `ExecStart` → `inspec-watchdog.sh` (replaces bare `run-scan.sh`) |

### Timer schedule (Deployment Engineer's)
- `spindle-chaos-agent.timer` — every 5m (inject drift)
- `spindle-inscan.timer` — every 2m → **now runs the watchdog** (detect + conditional repair)
- `spindle-cinc-client.timer` — every 10m (unconditional converge)

## Live trace (2026-08-10)

### Cycle A — direct watchdog run
| Time (UTC) | Event |
|---|---|
| 05:42:52.783 | chaos-web_app.sh injects Apache drift (`Listen 80→9090`, duplicate directive) |
| 05:43:17 | `inspec-watchdog.sh` starts on fleet-01 |
| 05:43:18 | Detected role `web_app` → profile `/tmp/spindle-qa/inspec/web` |
| 05:43:21 | **Scan: failed=3 total=5 skipped=0 (port-80 down + headers + symlink)** |
| 05:43:21 | **DEVIATION DETECTED → trigger converge** |
| 05:43:24–26 | converge: chefzero from `/var/chef`; `ports.conf` template restores `Listen 80`; apache reloaded; **9/31 resources updated** |
| 05:43:30 | Re-scan: **0 failed → REMEDIATED** |
| — | Final state: `Listen 80`, `http://localhost:80` = **HTTP 200**, `sites-enabled` = symlink |

### Cycle B — via systemd timer (autonomous, chaos re-injected by 5m timer)
| Time | Event |
|---|---|
| 05:46:18 | `spindle-inscan.service` triggered by timer |
| 05:46:21 | Scan: failed=3 (chaos had re-set `Listen 9090`) |
| 05:46:21–27 | Converge triggered, repairs (ports.conf → 80, apache reload) |
| 05:46:30 | Re-scan: 0 failed → **REMEDIATED**; unit deactivated cleanly |

This proves the intended dev-sec loop runs **fully unattended**: chaos injects →
watchdog detects → converge repairs → watchdog confirms clean.

## Findings / Fixes made along the way

### F1 — [FIXED] InSpec profiles failed to load (all 3 roles)
`controls/*.rb` duplicated the profile-level metadata header (`name`,
`title`, `depends`, …) that belongs only in `inspec.yml`, causing
`undefined method 'name'` at load. Stripped the invalid header from
`web`/`database`/`loadbalancer` control files (inspec.yml already declares the
metadata + `apache-baseline` dependency). Original files backed up to
`/tmp/spindle-qa/inspec_backup/`.

### F2 — [FIXED] Controls 01/02 used cinc-auditor-incompatible DSL
`host_inventory['hostname']` was evaluated at control scope (unavailable), and
`let(:headers)` is not a valid InSpec DSL construct. Rewrote drift-detection
controls (`spindle-web-01` port-80/HTTP, `spindle-web-02` security headers) to
plain `describe http(...)` / `.headers` access.

### F3 — [FIXED] Converge nil-deref in `web_app.rb`
`node['spindle_qa']['app_name']` threw `NoMethodError: undefined method '[]' for
nil` because no attributes file defines the `spindle_qa` namespace. Changed to
nil-safe `node.dig('spindle_qa', 'app_name')`. **(Pre-existing recipe bug — was
blocking all converges.)**

### F4 — [FIXED] Converge did not restore `Listen 80`
The `web_app` recipe managed the vhost but not `ports.conf`, so chaos' `Listen
9090` was never repaired → control-01 could never pass. Added
`apache-ports.conf.erb` (`Listen 80`) + a `template '/etc/apache2/ports.conf'`
step to the recipe.

### F5 — [FIXED] `sites-enabled` plain file vs symlink
control-04 requires `sites-enabled/spindle-enterprise.conf` to be a symlink, but
a leftover plain file made `a2ensite` skip (its `not_if` checked `File.exist?`).
Added a purge step (remove non-symlink) before `a2ensite` and changed the guard
to `File.symlink?`. (Pre-existing baseline drift.)

### F6 — [FIXED] Converge used the wrong cookbook cache root
The `-c /etc/cinc/client.rb` + `solo_mode` converge reads
`/etc/cinc/.../cache/cookbooks/` (populated from `/var/chef/cookbooks` at run
time), which is why syncing edits to `/var/chef/cookbooks/` is the correct
deployment target. Noted for future cookbook updates.

### F7 — [NOTE] `run-success/failure` converge noise
The cookbook no longer 412s (F3 + `-c` fix), so converges now emit clean
run_converge payloads to the proxy instead of `status=failure` noise (Phase 3
H2 resolved by this bridge).

## Verification & pre-[DONE] checklist

- [x] Watchdog detects deviation (failed=3) and triggers converge
- [x] Converge repairs (ports.conf → 80, apache reload, 9/31 resources)
- [x] Watchdog confirms clean (re-scan 0 failed → REMEDIATED)
- [x] Wired into systemd timer (inscan → watchdog); autonomous cycle proven
- [x] All fixes backed up on node; edits synced to `/var/chef/cookbooks`
- [x] Artifacts committed to repo (`tools/inspec-watchdog.sh`,
      `tools/inspec_json_status.py`, `tools/run-converge.sh`,
      `tools/inspec/web/controls/spindle_web.rb`, this doc)
- [ ] Pushed + [DONE] to Matrix

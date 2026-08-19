# Spindle v0.2.3 — User Acceptance Testing Full Validation Cycle

**Status:** Proposed validation cycle (this document). To be followed by:
1. A **revision** document, if any intentions are found unattainable; and
2. An **outcome** document (with the relative version number) recording the success or failure of the related testing.

**Purpose:** Validate that Spindle v0.2.3 correctly extracts, observes, and surfaces the CINC fleet's state across all three observation surfaces (REST, MCP, Dashboard) — and, critically, that it demonstrably **detects drift/divergence**, **observes re-convergence/remediation**, and **tracks out-of-compliance ↔ compliant transitions**.

**Scope:**
- **Fleet:** 8 role-diverse nodes — fleet-01 (nginx), fleet-02 (apache + FreshRSS), fleet-03 (apache + RSS-Bridge), fleet-04 (Glance), fleet-05 (RSSHub), fleet-06 (Miniflux), fleet-07/08 (plain).
- **Stack:** CINC Server (.11), DB (.12), auditor (.14), Spindle server + worker (.15), dashboard (.16).
- **Surfaces under test:** REST API, MCP server, Dashboard.

---

## Phase 0 — Baseline Establishment

**Summary:** Bring the cluster to a clean, known-good state on v0.2.3 so every later observation has a reference point.

- Confirm every binary reports **v0.2.3** (server, worker, migrate, dashboard, mcp).
- Confirm all services healthy (server / worker / dashboard active; DB reachable; no dead letters).
- **Full-fleet re-converge** (all 8 nodes) + **full-fleet auditor re-scan**.
- Record the baseline snapshot: node count, run count, per-node compliance, resource-event count, cookbook inventory.
- Confirm `run_list` is populated on **every** node (the new extraction feature).
- Confirm the baseline is **all-green** (zero failed controls).

## Phase 1 — Extraction / Ingest Surface

**Summary:** Prove the two ingest paths (data-collector converge + auditor compliance) capture complete, correct, Chef-wire-format data.

- Converge a node with a **deliberate change** (touch a managed file); verify the run records correct `node_name` / `run_id` / timing / status, and the changed resource appears in `resource_events` (type, name, action, duration).
- Converge a node with **zero changes**; verify it is skipped gracefully (not dead-lettered, no state corruption).
- Run an auditor scan; verify a compliance report is created with correct `profile_id` / `node_id` / status, **every** control result is captured, and `impact` is a float (the fixed decode).
- Verify `run_list`, `chef_environment`, `chef_server_fqdn`, `policy_group`, `policy_name` are all preserved (wire-format fidelity).
- Verify `run_list → role` derivation (nodes with `role[web]` vs `recipe[base]`).

## Phase 2 — REST Read / Query Surface

**Summary:** Exercise every GET endpoint, filter, and detail path; prove filters actually filter (no silent no-ops).

- List endpoints return correct totals (nodes = 8, runs, compliance, cookbooks, resource-events, waivers).
- Detail endpoints: `/v1/nodes/{id}`, `/v1/nodes/{id}/state`, `/v1/runs/{id}`, `/v1/runs/{id}/resource-events`, `/v1/compliance/reports/{id}`.
- Every filter field returns the right subset: `name`, `platform`, `role`, `status`, `chef_environment` (e.g. `platform=ubuntu` → 8, `name=fleet-02` → 1, `role=web` → the role'd nodes).
- Invalid filter fields are **rejected**, not silently ignored.
- Pagination (`limit` + `page_token`) round-trips correctly.
- Health: `/v1/health`, `/v1/health/metrics` (Prometheus text).
- Error cases: 404 (missing), 401 (bad token), 403 (unauthorized).
- `/openapi.json` lists all documented routes (27 paths).

## Phase 3 — MCP Surface

**Summary:** Prove an AI agent can answer real fleet questions through the 19 MCP tools, with no silent no-op filters.

- `tools/list` returns **19 tools** across 3 namespaces (query 11 / admin 5 / ops 3).
- Natural-language scenarios, each verified against the expected tool + result:
  - "Show me all the servers in the fleet" → `list_nodes` → 8.
  - "Show me fleet-02" → `list_nodes search` → 1.
  - "What is drifting?" → `detect_drift` → real data (not seed).
  - "How are resources distributed?" → `aggregate_resources`.
  - "Which nodes have compliance reports?" → `list_compliance_reports`.
  - "What cookbooks are deployed?" → `list_cookbooks`.
  - "What is the health / metrics?" → `health_check` + `get_metrics` (text, no JSON error).
- `list_nodes` filters (`platform` / `status` / `role` / `search`) actually filter.
- Admin tools (`create_waiver`, `revoke_waiver`, `run_backup`, `restore_backup`, `config_validate`) behave correctly.

## Phase 4 — Dashboard Surface

**Summary:** Prove the UI connects, renders live data, and navigates without bouncing to login.

- Connect with the API token; confirm the Fleet Dashboard renders 8 nodes + run + compliance data.
- Click **every** tab (Dashboard, Nodes, Runs, Compliance, Cookbooks) — none bounce to login.
- Verify node rows show name / platform / environment / policy-group / last-seen / status.
- Verify the dashboard reflects live changes (numbers update after a converge / scan).

## Phase 5 — Chaos / Drift Injection

**Summary:** Inject drift across diverse node roles and prove Spindle **detects** it (InSpec red + drift surfaced).

- Inject each of the 8 chaos types (service-stop, service-disable, config-corrupt, package-purge, permission-drift, port-shift, motd-corrupt, user-removal) across the fleet.
- After each injection:
  - Re-scan; verify the affected control flips to **failed** (InSpec red).
  - Verify `detect_drift` / `aggregates` reflect the change (the drifted resource appears).
  - Verify REST + MCP + Dashboard **all** show the node out-of-compliance.
- Verify the failure carries the right node + control + impact.
- Verify **unrelated nodes stay green** (drift is correctly attributed, not fleet-wide).

## Phase 6 — Re-converge / Remediation

**Summary:** Prove the CINC client remediates drift and Spindle observes the recovery.

- Re-converge the drifted node (`cinc-client --once`).
- Verify the remediated resource flips back to green (InSpec green).
- Verify `detect_drift` clears / reflects the correction.
- Verify all three surfaces show the node back to compliant.
- Verify compliance report history retains **both** the red and green states (append-only, no overwrite).

## Phase 7 — Compliance State Transitions

**Summary:** Exercise the full out-of-compliance ↔ compliant cycle end-to-end, multiple times, on different roles.

- For at least 3 role-diverse nodes (apache, nginx, Glance): **green → drift → red → reconverge → green**.
- Verify each transition is a distinct compliance report.
- Verify the failure → recovery delta is observable in REST (report history), MCP (`list_compliance_reports`), and Dashboard.

## Phase 8 — Retention / Data Lifecycle

**Summary:** Prove the retention cleanup prunes old data and is correctly gated.

- With `auto_cleanup = false`, verify **no deletion**.
- Enable `auto_cleanup` + a short `processed_retention_days`; verify old reports + control results are pruned (children-first, no orphans).
- Verify fresh reports are untouched.
- Verify `run_list` is preserved across retention (the recent fix).

## Phase 9 — Robustness / Edge Cases

**Summary:** Prove the system degrades gracefully rather than corrupting state.

- Malformed data-collector payloads → rejected cleanly.
- Duplicate converge (replay) → idempotent.
- Concurrent converges from multiple nodes → all ingested without loss.
- Auth: bad / expired token → 401; auditor vs data-collector token scopes enforced.
- Volume: converge all 8 nodes near-simultaneously + scan → zero dead letters, zero panics.

## Phase 10 — Regression + Acceptance Sign-off

**Summary:** Re-run the full surface matrix one final time and certify.

- Full REST filter + detail matrix re-pass.
- Full MCP scenario set re-pass.
- Dashboard navigation re-pass.
- Confirm zero dead-lettered jobs, zero panics, zero silent no-ops.
- **Accept / reject with evidence.**

---

## Acceptance Criteria (summary)

- All 8 nodes registered with `run_list` + full attributes.
- Every filter (REST + MCP) returns the correct subset.
- Drift is **detected, attributed to the right node, and surfaced on all three surfaces**.
- Re-converge **remediates and is observed**.
- Compliance transitions are **append-only and observable**.
- No panics, no silent no-ops, no dead letters — under clean *and* chaos conditions.

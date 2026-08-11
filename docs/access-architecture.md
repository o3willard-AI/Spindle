# Spindle Access Architecture — CLI, Web UI, MCP

> **Author:** Hephaestus  
> **Date:** 2026-08-10  
> **Purpose:** Define the three consumer interfaces for Spindle data access — how humans, programs, and agents interact with the system.

---

## 1. Why Three Interfaces?

Spindle's job is to surface infrastructure state. Different consumers have different needs:

| Consumer | Needs | Best Fit |
|----------|-------|----------|
| **Human operator** (fleet check) | Quick status, browse recent runs | Web UI |
| **Human operator** (debugging) | Raw data, scripting, pipes | CLI |
| **Program/script** (CI/CD) | Machine-readable output, exit codes | CLI (JSON mode) |
| **AI agent** (Hephaestus, etc.) | Tool-callable, context-efficient | MCP |
| **Automation** (cron, Nagios) | Structured output, zero interaction | CLI + exit codes |

All three share the same backend — the REST API at `:8080` — and the same auth (Bearer token). They differ only in presentation and protocol.

---

## 2. CLI — `spindle`

### 2.1 Design Principles

- **Progressive disclosure**: default output is a scannable table; `--json` flag switches to raw JSON for pipes
- **Exit codes as contract**: 0=success, 1=error, 2=API unavailable, 3=auth failure
- **Self-documenting**: `spindle help` and `spindle <command> --help` must be complete
- **Single binary**: `spindle` binary with subcommands, zero runtime deps beyond a TLS library
- **Token from env or config**: `SPINDLE_TOKEN` env var or `--token` flag; `spindle config set-token` for persistence

### 2.2 Command Tree

```
spindle
├── config                    # Local config management
│   ├── set-token <token>     # Persist auth token
│   ├── set-server <url>      # Persist server URL
│   ├── show                  # Show current config
│   └── validate              # Test connection + auth
│
├── nodes                     # Fleet inventory
│   ├── list                  # All nodes (table or JSON)
│   │   └── --platform, --status, --search filters
│   ├── show <id|name>        # Single node detail
│   └── state <id|name>       # Current state (last run, compliance)
│
├── runs                      # Converge history
│   ├── list                  # Recent runs
│   │   └── --node, --status, --since, --limit filters
│   ├── show <id>             # Full run detail
│   └── resources <id>        # Resource events for a run
│
├── compliance                # InSpec/compliance
│   ├── reports               # Reports list
│   │   └── --node, --profile, --status filters
│   ├── show <id>             # Report detail with controls
│   └── status <node>         # Per-node pass/fail/skip summary
│
├── cookbooks                 # Cookbook inventory
│   ├── list                  # All cookbooks + version counts
│   └── show <name>           # Version history
│
├── aggregates                # Rollup queries
│   └── resources             # Group-by cookbook/type/platform
│       └── --group-by, --window filters
│
├── drift                     # Change frequency
│   └── resources             # Frequently-changing resources
│       └── --window, --threshold, --node filters
│
├── health                    # System health
│   ├── status                # Quick check (DEEP if --deep)
│   └── metrics               # Prometheus metrics dump
│
├── export                    # Bulk data export
│   └── compliance <node>     # Full compliance history as JSONL
│
├── waivers                   # Waiver management (future)
│
└── backup                    # Operator tasks
    ├── create                # Full backup (DB + archive)
    └── restore <path>        # Restore from backup
```

### 2.3 Output Formats

**Table mode (default, for humans):**
```
$ spindle nodes list
NAME        PLATFORM    LAST SEEN            STATUS  
fleet-01    ubuntu      2026-08-10 08:54Z    compliant
fleet-02    ubuntu      2026-08-10 08:54Z    compliant
fleet-03    ubuntu      2026-08-10 08:54Z    compliant
```

**JSON mode (for pipes, scripts, agents):**
```
$ spindle nodes list --json
{"nodes":[{"name":"fleet-01","platform":"ubuntu",...}]}
```

**Error output (stderr, always JSON for parseability):**
```
$ spindle nodes show nonexistent
{"error":"NOT_FOUND","message":"Node 'nonexistent' not found","request_id":"abc123"}
[exit code 1]
```

### 2.4 Implementation Status

The `spindle-cli` crate already exists with:
- `commands.rs` — command definitions (clap derive)
- `client.rs` — HTTP client wrapping the REST API
- `config.rs` — `~/.spindle/config.toml` management
- `format_util.rs` — table and JSON output formatters
- `runner.rs` — command dispatch and error handling

**Gap:** Only covers a subset of the API. Need to extend to all endpoints above.

### 2.5 Agent-Friendly Design

Agents use the CLI by calling `spindle --json <command>`. Key affordances:
- `--json` flag on every command guarantees parseable output
- Exit codes are reliable (0/1/2/3) — agents don't need to parse error messages
- `SPINDLE_TOKEN` env var means no inline secrets in agent prompts
- All list commands support `--limit` to prevent context blowout

---

## 3. Web UI — Spindle Dashboard

### 3.1 Design Principles

- **Intuitiveness**: a new operator can understand fleet state in under 10 seconds
- **Usability**: common tasks in ≤3 clicks, ≤10 characters typed
- **Server-side rendering with minimal JS**: static HTML served by spindle-server, progressive enhancement with htmx or vanilla JS
- **Responsive**: works on desktop and mobile (operators checking from phone)
- **Dark mode by default**: infrastructure dashboards are read in dim server rooms
- **Zero new dependencies**: served as static files from the spindle-server binary; no npm build step for deployment

### 3.2 Page Map

```
/dashboard                 Fleet overview — node cards, health, recent runs
/nodes                     Node list with filtering
/nodes/:name               Node detail — attributes, run history, compliance
/runs                      Run history with filtering
/runs/:id                  Run detail — resources, timing, diff
/compliance                Compliance report list
/compliance/:id            Report detail — controls, results
/cookbooks                 Cookbook inventory
/cookbooks/:name           Cookbook version history
```

### 3.3 Dashboard Layout (Wireframe)

```
┌─────────────────────────────────────────────────────────┐
│  SPINDLE                          [healthy] ⚡ v0.1.0    │
│─────────────────────────────────────────────────────────│
│  Fleet Status                       Last Ingest: 3s ago │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐              │
│  │ fleet-01  │  │ fleet-02  │  │ fleet-03  │              │
│  │ ✅ compliant│  │ ✅ compliant│  │ ✅ compliant│              │
│  │ ubuntu    │  │ ubuntu    │  │ ubuntu    │              │
│  │ 5m ago    │  │ 5m ago    │  │ 5m ago    │              │
│  └──────────┘  └──────────┘  └──────────┘              │
│─────────────────────────────────────────────────────────│
│  Recent Runs                                            │
│  fleet-01 · converge · 08:54 · 5/30 updated · 2.1s     │
│  fleet-02 · converge · 08:52 · 0/22 updated · 1.8s     │
│  fleet-03 · converge · 08:49 · 6/18 updated · 3.4s     │
│─────────────────────────────────────────────────────────│
│  Compliance                                             │
│  Reports: 3 · Pass: 54 · Fail: 0 · Skip: 0             │
└─────────────────────────────────────────────────────────┘
```

### 3.4 Technical Approach

**Option A: Pure server-rendered HTML (recommended)**
- `spindle-server` serves static HTML/CSS at `/`
- Each page is a Rust template (askama or tera) rendered server-side
- htmx for dynamic updates (health polling, auto-refresh)
- Zero JavaScript build pipeline — templates compile into the binary
- Styles: a single CSS file (~500 lines), dark theme, system font stack

**Option B: SPA (React/Vue) — NOT recommended**
- Requires a build step, npm, bundler
- Heavier deployment (static files or separate service)
- Overkill for a dashboard with 8 pages

**Recommendation: Option A.** The dashboard is a read-only view of API data. Server-side rendering with htmx for live updates is simpler, faster to build, and has zero operational overhead.

### 3.5 UX Metrics

| Metric | Target |
|--------|--------|
| Time to understand fleet state (new user) | < 10 seconds |
| Clicks to check a failed run | ≤ 3 |
| Characters typed to find a node | ≤ 10 (search-as-you-type) |
| Page load time (dashboard) | < 500ms |
| Mobile usability | All pages readable at 375px width |

---

## 4. MCP — Spindle Tools for Agents

### 4.1 Design Principles

- **Tool sharding by concern**: split tools into namespaces to keep context budgets manageable
- **Security at the transport layer**: token required; each tool declares required auth scope
- **Self-describing**: every tool returns enough context in its output for an agent to decide next steps without re-calling
- **Convention over configuration**: default page sizes, default windows — agents shouldn't need to specify everything

### 4.2 Tool Sharding Strategy

Three namespaces, each ~5-8 tools. An agent loads only the namespace it needs.

```
spindle-query (read-only)          spindle-admin (mutating)         spindle-ops (health/meta)
├── list_nodes                     ├── create_waiver                ├── health_check
├── get_node                       ├── revoke_waiver                ├── get_metrics
├── list_runs                      ├── run_backup                   ├── ingest_lag
├── get_run                        ├── restore_backup               └── queue_depth
├── list_resource_events           └── config_validate
├── list_compliance_reports
├── get_compliance_report
├── list_cookbooks
├── get_cookbook
├── aggregate_resources
└── detect_drift
```

**Sharding rationale:**
- `spindle-query` is the most-used namespace — agents checking fleet state, investigating runs, browsing compliance
- `spindle-admin` is rarely needed and mutates state — separate to reduce blast radius
- `spindle-ops` is for monitoring/alerting agents — small, focused, non-overlapping with query

Each namespace maps to its own MCP server instance. Agents configure which servers to connect to.

### 4.3 Tool Design Conventions

Every tool returns structured JSON with these fields:
```json
{
  "data": [...],           // The actual results
  "pagination": {          // Always present on list tools
    "total": 47,
    "page": 1,
    "page_size": 20,
    "has_more": true
  },
  "summary": "3 nodes: 2 compliant, 1 failing",  // Human-readable for agent reasoning
  "request_id": "abc123"
}
```

**Anti-patterns avoided:**
- No tool returns raw HTML (always JSON)
- No tool requires multi-step pagination from the agent (default page_size=20, agents can override with `--limit`)
- No tool accepts raw SQL (injection vector)
- All timestamps in ISO 8601 (RFC 3339)

### 4.4 Security Model

| Layer | Mechanism |
|-------|-----------|
| **Transport** | HTTPS (or localhost for stdio transport) |
| **Authentication** | Bearer token in `SPINDLE_TOKEN` env var or `--token` parameter |
| **Authorization** | Read-only tools require `query` scope; mutating tools require `admin` scope |
| **Audit** | Every tool call logged with request_id, tool name, caller identity |
| **Rate limiting** | Per-tool rate limits prevent context-window abuse (e.g., max 60 calls/min for query tools) |
| **Token scoping** | Tokens can be scoped to specific namespaces (`spindle-query` only) |

**Open question:** MCP's security model is still evolving. The current best practice is to run MCP servers locally (stdio transport) where the caller's OS identity provides implicit auth. Remote MCP (HTTP) needs explicit token auth. We implement both.

### 4.5 Implementation Approach

**Transport:** stdio (local) + optional HTTP (remote). The stdio transport is the primary path — agents run `spindle-mcp` locally.

**Architecture:** A single binary `spindle-mcp` that can run in different server modes:
```bash
# Query server (read-only)
spindle-mcp serve --namespace spindle-query --token $SPINDLE_TOKEN

# Admin server (mutating)
spindle-mcp serve --namespace spindle-admin --token $SPINDLE_TOKEN

# Ops server (health/meta)
spindle-mcp serve --namespace spindle-ops --token $SPINDLE_TOKEN
```

**Hermes config example:**
```yaml
mcp_servers:
  spindle-query:
    command: spindle-mcp
    args: ["serve", "--namespace", "spindle-query"]
    env:
      SPINDLE_TOKEN: "${SPINDLE_TOKEN}"
  spindle-admin:
    command: spindle-mcp
    args: ["serve", "--namespace", "spindle-admin"]
    env:
      SPINDLE_TOKEN: "${SPINDLE_TOKEN}"
```

### 4.6 Context Budget Analysis

| Namespace | Tool count | Max input tokens (estimated) | Max output tokens (typical page) |
|-----------|------------|------------------------------|----------------------------------|
| spindle-query | 11 | ~8,000 | ~3,000 |
| spindle-admin | 5 | ~3,000 | ~1,500 |
| spindle-ops | 3 | ~1,500 | ~500 |

Each namespace fits comfortably within a 128K context window alongside other tools. An agent loading all three would use ~13K tokens for tool definitions — acceptable but unnecessary. The sharding lets agents be selective.

---

## 5. Shared Infrastructure

### 5.1 Auth

All three interfaces use the same Bearer token auth. A single `spindle-dev-token` (or user-generated token) works across CLI, Web UI, and MCP.

**Token management:**
```
CLI:     SPINDLE_TOKEN env var → ~/.spindle/config.toml → --token flag
Web UI:  Browser session storage (entered on login page)
MCP:     SPINDLE_TOKEN env var in MCP server config
```

### 5.2 Request ID Propagation

Every request through any interface gets a unique `request_id` (UUIDv7) that propagates through:
- Ingress → API handler → log output → response header (`X-Request-Id`)
- This lets operators correlate a CLI command, a Web UI page load, or an MCP tool call with specific log entries

### 5.3 Error Envelope

All three interfaces share the same error contract:
```json
{
  "error": {
    "code": "NOT_FOUND",
    "message": "Node 'fleet-99' not found",
    "details": {"requested_id": "fleet-99"},
    "request_id": "abc123"
  }
}
```

CLI formats this to stderr. Web UI renders it in an error banner. MCP returns it as the tool output (agent can parse `error.code`).

---

## 6. Implementation Plan

### Phase 1 — CLI Completion (Mike)
- Extend `spindle-cli` to cover all API endpoints
- Add `--json` flag to all commands
- Add `--limit`, `--since`, filter flags
- Test: every command works against live .101

### Phase 2 — MCP Server (Sergey)
- New crate: `spindle-mcp` (or integrate into `spindle-cli`)
- stdio + HTTP transports
- Three namespace servers
- Test: Hermes MCP client discovers spindle-query tools and runs `list_nodes`

### Phase 3 — Web Dashboard (Mark)
- Server-side rendered HTML with askama templates
- Served by `spindle-server` at `/`
- htmx for live health polling
- Test: load `/dashboard` in browser → see fleet status

### Phase 4 — Integration
- Auth consistency across all three
- Request ID propagation verified end-to-end
- Documentation: `docs/access-architecture.md` (this document) + `docs/cli-reference.md` + `docs/mcp-tools.md`

---

## 7. Open Questions

1. **MCP tool auth scoping**: Should we implement token scopes now (query vs admin) or defer? Recommendation: implement the namespace split first, add scope enforcement later.
2. **Web UI auth**: Session-based (cookie) or token-based (localStorage)? Recommendation: token in localStorage — simpler, matches CLI/MCP pattern, no session state on server.
3. **CLI output**: Table mode needs a Rust library. Options: `tabled` (popular), `comfy-table` (lightweight), or manual column formatting. Recommendation: `tabled` — feature-complete, active maintenance.
4. **Web UI framework**: askama (compile-time templates) vs tera (runtime templates). Recommendation: askama — compile-time type checking catches template errors at build time.

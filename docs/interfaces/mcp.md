# Spindle MCP Server Reference

The Spindle MCP (Model Context Protocol) server exposes the Spindle REST API as
tools consumable by any MCP-compatible client (Claude Desktop, Cursor, custom
agents). Communication is over **stdio** using newline-delimited JSON-RPC 2.0.

## Quick Start

```bash
# Start the MCP server (stdio transport)
spindle-mcp serve --namespace spindle-query --api-url http://127.0.0.1:3000 --token spindle-dev-token
```

On startup, the server logs to stderr:
```
spindle-mcp: serving spindle-query against http://127.0.0.1:3000 (11 tools, stdio)
```

## Namespaces

The MCP server exposes three namespaces, each with a different toolset:

| Namespace | Tools | Access | Description |
|---|---|---|---|
| `spindle-query` | 11 | Read-only (GET) | Query nodes, runs, compliance, cookbooks, waivers, resource events |
| `spindle-admin` | 5 | Read/write | Manage waivers, dead-letter queue, pipeline triggers |
| `spindle-ops` | 3 | Operational | Health checks, metrics, backup status |

## Wire Protocol

- **Transport**: stdio (stdin for client→server, stdout for server→client)
- **Format**: JSON-RPC 2.0, one message per line (newline-delimited)
- **Empty lines**: skipped
- **EOF on stdin**: server exits cleanly

### Supported JSON-RPC methods

| Method | Response | Notes |
|---|---|---|
| `initialize` | `{protocolVersion, capabilities, serverInfo}` | Handshake — call first |
| `tools/list` | `{tools: [...]}` | Returns all tools for the namespace |
| `tools/call` | `{content: [...], structuredContent: {...}}` | Executes a tool |
| `ping` | `{}` | Health check |

## Tool Catalog

### spindle-query (11 tools)

| # | Tool | Parameters | API Endpoint | Description |
|---|---|---|---|---|
| 1 | `list_nodes` | `limit`, `platform`, `status`, `search` | `GET /v1/nodes` | List fleet nodes with filters |
| 2 | `get_node` | `id` (UUID) | `GET /v1/nodes/:id` | Get node details |
| 3 | `node_state` | `id` (UUID) | `GET /v1/nodes/:id/state` | Get node state snapshot |
| 4 | `list_runs` | `limit`, `node_id`, `start_time`, `end_time` | `GET /v1/runs` | List run history |
| 5 | `get_run` | `id` (UUID) | `GET /v1/runs/:id` | Get run details |
| 6 | `run_events` | `run_id` (UUID) | `GET /v1/runs/:id/events` | List resource events for a run |
| 7 | `list_compliance_reports` | `limit`, `node`, `profile` | `GET /v1/compliance/reports` | List compliance reports |
| 8 | `get_compliance_report` | `id` (UUID) | `GET /v1/compliance/reports/:id` | Get a single compliance report |
| 9 | `list_cookbooks` | — | `GET /v1/cookbooks` | List cookbook inventory |
| 10 | `list_waivers` | — | `GET /v1/waivers` | List active compliance waivers |
| 11 | `detect_drift` | `window`, `threshold`, `node` | `GET /v1/resource-events/drift` | Detect resource drift |

### spindle-admin (5 tools)

| # | Tool | Parameters | API Endpoint | Description |
|---|---|---|---|---|
| 1 | `create_waiver` | `control_id`, `profile_id`, `scope`, `scope_value`, `justification`, `approver`, `start_date`, `expiry_date` | `POST /v1/waivers` | Create a compliance waiver |
| 2 | `update_waiver` | `id`, `expiry_date`, `justification` | `PUT /v1/waivers/:id` | Update an existing waiver |
| 3 | `delete_waiver` | `id` | `DELETE /v1/waivers/:id` | Revoke a waiver |
| 4 | `list_dead_letter` | `limit`, `offset` | `GET /v1/admin/dead-letter` | List dead-letter queue entries |
| 5 | `trigger_pipeline` | `archive_key` | `POST /v1/admin/process-payload` | Trigger one-shot pipeline processing |

### spindle-ops (3 tools)

| # | Tool | Parameters | API Endpoint | Description |
|---|---|---|---|---|
| 1 | `health_check` | — | `GET /health` | Get subsystem health status |
| 2 | `get_metrics` | — | `GET /metrics` | Get Prometheus metrics text |
| 3 | `backup_status` | — | `GET /v1/admin/backup-status` | Get backup status info |

## Response Envelope

All tool responses return a standard envelope in `structuredContent`:

```json
{
  "data": [ ... ],
  "pagination": { "limit": 50, "offset": 0, "has_more": false },
  "summary": "Listed 5 node(s) — /v1/nodes (5 items)",
  "request_id": "uuid"
}
```

On API errors, the envelope still returns (with error info in `data`), so
callers always get a consistent structure.

## Sample Client Session

### 1. Initialize handshake

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"my-client","version":"1.0.0"}}}
```

**Response:**
```json
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"spindle-mcp","version":"0.2.0"}}}
```

### 2. List available tools

```json
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
```

**Response (truncated):**
```json
{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"list_nodes","description":"List fleet nodes...","inputSchema":{"type":"object","properties":{"limit":{"type":"integer"},...}}},...]}}
```

### 3. Query: list nodes

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_nodes","arguments":{"limit":5,"platform":"ubuntu"}}}
```

**Response:**
```json
{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"..."}],"structuredContent":{"data":[{"id":"3f9f50a9-...","name":"web-server-01","platform":"ubuntu","status":"compliant"}],"pagination":{"limit":5,"offset":0,"has_more":true},"summary":"Listed 5 node(s) — /v1/nodes (5 items)","request_id":"req-uuid"}}}
```

### 4. Admin action: create a waiver

Switch to the `spindle-admin` namespace:

```bash
spindle-mcp serve --namespace spindle-admin --api-url http://127.0.0.1:3000 --token <admin-jwt>
```

```json
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"create_waiver","arguments":{"control_id":"cis-1.1","profile_id":"cis-baseline","scope":"node","scope_value":"web-server-01","justification":"Compensating control","approver":"sec-team","start_date":"2026-08-13","expiry_date":"2026-09-13"}}}
```

**Response:**
```json
{"jsonrpc":"2.0","id":4,"result":{"content":[{"type":"text","text":"..."}],"structuredContent":{"data":{"id":"new-waiver-uuid","control_id":"cis-1.1","is_expired":false},"summary":"Waiver created","request_id":"req-uuid"}}}
```

## Claude Desktop Integration

Add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "spindle": {
      "command": "spindle-mcp",
      "args": ["serve", "--namespace", "spindle-query", "--api-url", "http://127.0.0.1:3000"],
      "env": {
        "SPINDLE_TOKEN": "spindle-dev-token"
      }
    }
  }
}
```

## CLI Flags

| Flag | Description |
|---|---|
| `serve` | Subcommand to start the stdio server |
| `--namespace <ns>` | Tool namespace: `spindle-query`, `spindle-admin`, `spindle-ops` |
| `--api-url <url>` | Spindle REST API URL |
| `--token <token>` | Bearer token (or use `SPINDLE_TOKEN` env var) |

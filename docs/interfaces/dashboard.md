# Spindle Dashboard Reference

The Spindle dashboard (`spindle-dashboard`) is a stateless, server-rendered web
UI built with **Axum + Askama templates**. It provides a browser-based interface
for browsing fleet data without writing API calls.

## Architecture

- **Stateless**: no session state — horizontally scalable behind a load balancer
- **Server-rendered**: HTML pages generated with Askama templates
- **API proxy**: every request proxies the caller's bearer token to the Spindle
  REST API (no data stored in the dashboard process)
- **API URL resolution**: `--api-url` CLI flag → `SPINDLE_API_URL` env → fallback
  `http://127.0.0.1:8080`
- **Default port**: 3000
- **HTTP client**: `reqwest::Client` with 15s timeout

## Starting the Dashboard

```bash
# Standalone
spindle-dashboard --api-url http://127.0.0.1:3000 --port 3001

# Via env var
SPINDLE_API_URL=http://127.0.0.1:3000 spindle-dashboard
```

## Authentication

The dashboard does not manage its own authentication. Instead, it proxies the
caller's bearer token:

1. **Browser** sends `X-Api-Token: <token>` or `Authorization: Bearer <token>`
2. **Dashboard** forwards the token to the Spindle REST API
3. If no token is provided, API calls return `401` and the dashboard shows a
   login prompt

## Pages & Views

### Dashboard Home (`/`)

Overview page showing:
- Fleet summary: total nodes, nodes by status (compliant/failed/unknown)
- Recent runs (last 10)
- Active compliance waivers count
- Health status (database, storage, Dex)

### Nodes (`/nodes`)

- **Node list**: table with name, platform, status, last seen, project
- **Filters**: platform, status, search (client-side filtering)
- **Node detail** (`/nodes/:id`): full node information including run history,
  compliance status, and cookbook assignments

### Runs (`/runs`)

- **Run list**: table with node name, run ID, status, start/end time, duration
- **Run detail** (`/runs/:id`): full run information including resource events
  and error details for failed runs

### Compliance (`/compliance`)

- **Report list**: table with node, profile, status, controls passed/failed
- **Report detail** (`/compliance/reports/:id`): full control results with
  pass/fail/skip status per control

### Waivers (`/waivers`)

- **Waiver list**: table with control ID, profile, scope, expiry, approver
- **Create waiver** (`/waivers/new`): form for creating a new waiver (requires
  admin role)
- **Waiver detail** (`/waivers/:id`): full waiver information with audit history

### Cookbooks (`/cookbooks`)

- **Cookbook list**: table with cookbook name, versions, node count per version
- **Cookbook detail** (`/cookbooks/:name`): version breakdown and nodes using
  each version

### Resource Events (`/resources`)

- **Aggregates**: bar chart of resource events grouped by cookbook over time
- **Drift detection**: table of resources with significant changes across runs

## API Proxying

The dashboard makes API calls using the caller's token:

```
Browser → Dashboard (X-Api-Token: xxx)
                → Spindle API (Authorization: Bearer xxx)
                ← JSON response
        ← HTML page (rendered with data)
```

This means:
- The dashboard can be deployed anywhere (no database needed)
- Token validation happens at the API layer, not the dashboard
- Role-based access control is enforced by the API

## Configuration

| Setting | CLI Flag | Env Var | Default |
|---|---|---|---|
| API URL | `--api-url` | `SPINDLE_API_URL` | `http://127.0.0.1:8080` |
| Port | `--port` | `SPINDLE_DASHBOARD_PORT` | `3000` |
| Bind address | `--host` | `SPINDLE_DASHBOARD_HOST` | `0.0.0.0` |

## Deployment

The dashboard is included in the deployment bundle and can run as a separate
systemd service:

```ini
[Unit]
Description=Spindle Dashboard
After=network.target

[Service]
Type=simple
User=spindle
ExecStart=/opt/spindle/bin/spindle-dashboard --api-url http://127.0.0.1:3000
Environment=SPINDLE_API_URL=http://127.0.0.1:3000
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

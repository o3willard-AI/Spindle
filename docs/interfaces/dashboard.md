# Spindle Dashboard Reference

The Spindle dashboard (`spindle-dashboard`) is a single-binary web console for
the Spindle REST API. It serves an embedded React SPA and reverse-proxies
`/v1/*` requests to the API, so the browser can talk to the fleet data
same-origin.

<!-- screenshots: added in a follow-up task -->

## Architecture

The dashboard is a **client-rendered** single-page application compiled to
static assets and embedded into the binary at build time via `rust-embed`.
There is no server-side template rendering — the SPA fetches data from the
API over the same origin and renders in the browser.

- **Embedded SPA**: `frontend/dist/` (Vite build output) is compiled into the
  binary with `rust-embed`. One artifact, no separate static-file server.
- **Client-rendered**: all pages are React components. The server serves
  `index.html` for every non-API path (SPA fallback for deep links), and the
  browser router handles the rest.
- **API proxy**: every `/v1/*` request is forwarded to the Spindle API with
  the caller's auth headers (`X-Api-Token` / `Authorization: Bearer`) passed
  through untouched. The dashboard process holds no session state — tokens
  live in the browser's `localStorage`.
- **Stateless**: no server-side sessions. Multiple dashboard instances can be
  load-balanced behind nginx / Apache / HAProxy.
- **HTTP client**: `reqwest::Client` with a 15 s timeout for upstream API
  calls.

### Frontend stack

| Layer | Technology |
|---|---|
| Build | Vite |
| Framework | React + TypeScript |
| Routing | TanStack Router (file-based, `frontend/src/routes/`) |
| Styling | Tailwind CSS |
| UI kit | shadcn/ui |
| Data fetching | TanStack Query |
| Search | Command dialog (Ctrl/⌘ + K) |

## Starting the Dashboard

```bash
# Standalone (explicit API URL)
spindle-dashboard --api-url http://127.0.0.1:3000 --port 3001

# Via environment variable
SPINDLE_API_URL=http://127.0.0.1:3000 spindle-dashboard
```

The binary always listens on `0.0.0.0`.

### Runtime flags

| Flag | Description | Default |
|---|---|---|
| `--port` | TCP port to listen on | `3000` |
| `--api-url` | Spindle REST API base URL. Overrides `SPINDLE_API_URL`. | — |

**API URL resolution order:** `--api-url` flag → `SPINDLE_API_URL` env var →
fallback `http://127.0.0.1:8080`. Trailing slashes are stripped.

## Authentication

The SPA keeps the API token in the browser's `localStorage` (key
`spindle_token`). Every fetch attaches the token via **both** `X-Api-Token`
and `Authorization: Bearer` headers, so both legacy and current API middleware
configurations are accepted.

Flow:

```
Browser  ──X-Api-Token: <token>──►  Dashboard (proxy)  ──Authorization: Bearer <token>──►  Spindle API
Browser  ◄──JSON response────────  Dashboard          ◄──JSON response─────────────────
```

- **No server-side sessions.** The dashboard forwards the token verbatim.
- **Token validation** happens at the API layer, not the dashboard.
- **RBAC** is enforced by the API (see [identity.md](identity.md)).
- When no token is set, API calls return `401` and pages show a
  "check your API token" empty state.

## Pages & Views

The SPA uses file-based routing under `frontend/src/routes/`. The sidebar
navigation lists: Dashboard, Nodes, Converge runs, Compliance, Profiles,
Cookbooks.

### Dashboard Home (`/`)

Fleet overview:

- Fleet summary: total nodes, nodes by status (compliant / failed / unknown)
- Recent runs
- Node compliance breakdown

### Nodes (`/nodes`)

- **Node list**: table with name, platform, status, last seen, environment
- **Node detail** (`/nodes/<id>`): full node information including run
  history, compliance status, and cookbook assignments

### Runs (`/runs`)

- **Run list**: table with node, run ID, status, start / end time, duration
- **Run detail** (`/runs/<id>`): full run information including resource
  events and error details for failed runs

### Compliance (`/compliance`)

- Compliance report list with node, profile, status, controls
  passed / failed
- Control-level results with pass / fail / warn status

### Profiles (`/profiles`)

- **Profile list** (`/profiles/`): compliance profiles with control counts
- **Profile detail** (`/profiles/<id>`): individual controls and their
  latest results across nodes

### Cookbooks (`/cookbooks`)

- **Cookbook list**: cookbook name, versions, node count per version
- **Cookbook detail** (`/cookbooks/<name>`): version breakdown and the nodes
  running each version

### Settings (`/__spindle-admin/settings`)

Admin settings surface at a non-obvious URL. The page contains placeholder
panels (Users, Teams, API tokens, Notifications, Data lifecycle, System
health, Compliance waivers) that display "integration pending" — no API
calls are made. Admin endpoints are not yet wired up; RBAC and identity
provider integration are stubbed.

## Backend APIs Without a Dedicated Page

The following API endpoints exist and are functional but have **no
dedicated SPA page**:

- `GET /v1/waivers` — compliance waivers
- `GET /v1/resource-events/aggregates` — resource event aggregates
- `GET /v1/resource-events/drift` — drift detection

These can be called directly against the API (see
[http-api.md](http-api.md)).

## API Proxying

The dashboard proxies `/v1/*` to the Spindle API. Request method, path,
query string, body, `Content-Type`, `Accept`, and auth headers are forwarded
verbatim. The API's status code, content type, and body are returned
unchanged, so the SPA sees the same envelopes it would get from a direct API
connection.

```
Browser → Dashboard (/v1/nodes, X-Api-Token: xxx)
                → Spindle API (Authorization: Bearer xxx)
                ← JSON response
        ← JSON response (same envelope)
```

If the API is unreachable, the proxy returns `502` with an
`api_unreachable` or `upstream_read_failed` error code.

## Configuration

| Setting | CLI flag | Env var | Default |
|---|---|---|---|
| API URL | `--api-url` | `SPINDLE_API_URL` | `http://127.0.0.1:8080` |
| Port | `--port` | — | `3000` |

The binary binds to `0.0.0.0`. Use a reverse proxy or firewall to restrict
access.

## Deployment

The dashboard ships as a single binary (frontend assets embedded). It can run
as a systemd service:

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

Multiple instances can run behind a load balancer; the process is stateless.

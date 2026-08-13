# Spindle Interface Documentation

Spindle exposes six interfaces for operators, developers, and external systems.
Each document below provides a complete reference with working examples grounded
in the actual source code.

| # | Interface | Document | Description |
|---|---|---|---|
| 1 | HTTP REST API | [http-api.md](http-api.md) | Every route group with curl examples, auth, request/response shapes, OpenAPI/Swagger UI |
| 2 | MCP Server | [mcp.md](mcp.md) | Full tool catalog (11 query + 5 admin + 3 ops), stdio transport, sample client sessions |
| 3 | CLI | [cli.md](cli.md) | All subcommands with flags, arguments, and multi-step workflows |
| 4 | Identity & Auth | [identity.md](identity.md) | SAML, LDAP, OIDC/Dex, JWKS, JIT provisioning, local accounts — config blocks and auth flows |
| 5 | Dashboard | [dashboard.md](dashboard.md) | Web UI pages, views, deployment, API proxying |
| 6 | Metrics | [metrics.md](metrics.md) | Every Prometheus metric: name, type, labels, description, scrape config, Grafana queries |

## Quick Links

- **API base URL**: `http://127.0.0.1:3000` (default)
- **Interactive API docs**: `http://127.0.0.1:3000/docs` (Swagger UI)
- **OpenAPI spec**: `http://127.0.0.1:3000/openapi.json`
- **Health check**: `GET /health`
- **Metrics**: `GET /metrics`
- **Default token**: `spindle-dev-token` (override with `SPINDLE_INGEST_TOKEN`)

## Related Documentation

- [Backup & Restore](../operator/backup-restore.md) — script-based backup procedures
- [Quick Start](../operator/quick-start.md) — getting started guide
- [Engineering Spec](../spec/spindle-engineering-spec.md) — architecture and design
- [Security Audit](../uat/security-audit.md) — security findings and remediations

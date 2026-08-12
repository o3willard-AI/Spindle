# ADR-001: Spindle Security Baseline

## Status
Accepted

## Context

Spindle ingests fleet infrastructure data (Chef Infra data-collector events,
InSpec compliance reports) and exposes it via a REST API. The system handles
sensitive infrastructure data and must enforce strong authentication,
authorization, and data-protection controls.

This ADR documents the security architecture established in audit Phase 5.A.1
(Security Baseline).

## Decision

### Authentication

1. **Ingest API**: Bearer token auth via `SPINDLE_INGEST_TOKEN` environment
   variable. Default: `spindle-dev-token` (development only). In production,
   this must be set to a strong secret.

2. **Query API**: Same bearer token auth as ingest. All `/v1/*` endpoints require
   `Bearer <token>` via `require_bearer_token` middleware.

3. **OIDC/OAuth2**: JIT (Just-In-Time) provisioning via Dex. The OIDC issuer URL
   is configured in `config.toml` under `[identity]`. User claims are mapped
   to Spindle roles via `spindle-identity` crate.

### Authorization

1. **Scope-based**: Each bearer token has a scope (e.g., `ingest:run`, `query:nodes`).
   The `spindle-authz` crate enforces scope checks on every request.

2. **Role-based**: OIDC users get roles mapped from JWT claims (e.g., `admin`,
   `operator`, `viewer`). Admins can access all endpoints; viewers are limited
   to read-only query routes.

### Data Protection

1. **At rest**: PostgreSQL is the system of record. Archive payloads are stored
   on the local filesystem (or S3/MinIO) under `/var/lib/spindle/archive/` with
   gzip compression (ADR-003). PostgreSQL data directory should be encrypted at
   the OS level (LUKS).

2. **In transit**: All HTTP endpoints should be behind a TLS-terminating reverse
   proxy (nginx/caddy). The `spindle-server` itself does not terminate TLS.

3. **Secrets**: Never committed to git. `.env` and `configs/*.local.toml` are
   gitignored. Production secrets are managed via the KeePass vault (see
   memory notes) or a secrets manager.

### Network Security

1. **Listen address**: Defaults to `127.0.0.1:3000`. Should be behind a reverse
   proxy in production.

2. **CORS**: Disabled by default. Enable only if a specific origin is trusted.

3. **Rate limiting**: In-progress (audit P1-5 notes missing rate limiting on
   auth endpoints). See future ADR.

## Consequences

- Strong authentication is enforced on all non-health endpoints.
- OIDC integration adds complexity but provides audit trails via the `waiver_audit`
  table.
- TLS must be handled by an external proxy — `spindle-server` itself is not
  responsible for TLS termination.

# Spindle Identity & Authentication Reference

Spindle supports multiple identity integration patterns. Each can be used
independently or combined (e.g., Dex fronting multiple connectors, with JIT
provisioning into PostgreSQL).

## Overview

| Method | Crate | Config | Use Case |
|---|---|---|---|
| JIT OIDC Login | `spindle-server/jit_auth.rs` | `[identity]` in config | Primary: Dex → JIT provision users + roles |
| Local Accounts | `spindle-server/local_accounts.rs` | Env vars | Bootstrap admin, offline/airgapped |
| SAML SSO | `spindle-saml`, `spindle-dex` | `[identity.saml]` | Enterprise SSO via SAML 2.0 |
| LDAP | `spindle-dex` | `[identity.ldap]` | Active Directory / OpenLDAP |
| OIDC/Dex | `spindle-dex` | `[identity.oidc]` | Centralized IdP with Dex |
| JWKS | `spindle-server/jwk.rs` | `SPINDLE_JWKS_URL` | External JWT validation via JWKS |

---

## 1. JIT OIDC Login (Primary Auth)

JIT (Just-In-Time) provisioning: when a user authenticates via an external IdP,
Spindle creates the user in PostgreSQL, evaluates mapping rules to assign roles,
and issues session JWTs — all in one transaction.

### Configuration

```toml
[identity]
issuer_url = "http://dex:5556"
client_id = "spindle"
client_secret = "CHANGE_ME"

[[identity.mappings]]
connector = "oidc"
groups = ["engineers", "admins"]
roles = ["admin", "viewer"]
scope = "acme,globex"
```

### Auth Flow (sequence)

1. **Client** → `GET /v1/auth/login?connector=oidc&subject=user@example.com&groups=engineers`
2. **Spindle** looks up the user by `(subject, connector)` in PostgreSQL
3. If not found → `INSERT INTO users (subject, connector, email, display_name, groups)` (atomic)
4. **Spindle** evaluates `MappingEvaluator` rules → assigns roles (`admin`, `viewer`)
5. Roles inserted into `user_roles` in the same transaction
6. **Spindle** issues HS256 access + refresh JWTs (secret from `SPINDLE_JWT_SECRET`)
7. **Client** receives `{access_token, refresh_token, token_type, expires_in}`
8. Subsequent requests use `Authorization: Bearer <access_token>`

### Key Points

- The `connector` parameter (`oidc`, `saml`, `ldap`, `local`) determines which
  IdP the user came from. Same subject + different connector = separate user records.
- Mapping rules are evaluated in order; first match wins. Empty connector = matches all.
- `groups` is a comma-separated list of group memberships from the IdP.
- JWTs are validated by `require_jwt_role` middleware on all protected routes.

---

## 2. Local Accounts

In-memory username/password store for bootstrap admin and airgapped deployments.
No external IdP required.

### Configuration (env vars)

| Env Var | Description |
|---|---|
| `SPINDLE_BOOTSTRAP_ADMIN_USER` | Bootstrap admin username (first-run only) |
| `SPINDLE_BOOTSTRAP_ADMIN_PASSWORD` | Bootstrap admin password (first-run only) |
| `SPINDLE_AUTH_RATE_LIMIT` | Max login attempts per minute (default: 10) |

### Auth Flow

1. **First run**: Spindle creates a bootstrap admin from env vars. Password is
   hashed with Argon2id and stored in memory. Password is cleared from env after creation.
2. **Login**: `POST /v1/auth/local/login` with `{username, password}`
3. **Spindle** verifies password against Argon2id hash
4. On success: resets failed attempts counter, issues JWT
5. On failure: increments `failed_attempts`. After `max_failed_attempts` → account locked for `lockout_duration_secs`

### Endpoints

- `POST /v1/auth/local/login` — authenticate, receive JWT
- `POST /v1/auth/local/register` — register new account (admin only)
- `GET /v1/auth/local/audit` — view audit log (admin only)

### Features

- Password rotation: `is_password_expired()` checks `password_changed_at + max_age_days`
- Account lockout: configurable via env vars
- Audit log: records `LocalLoginSuccess`, `LocalLoginFailed`, `LocalAccountLocked`, `BootstrapAdminCreated`

---

## 3. SAML SSO

SAML 2.0 Service Provider-initiated SSO with assertion validation.

### Configuration

```toml
[identity.saml]
client_id = "spindle-saml"
client_secret = "CHANGE_ME"
issuer = "https://spindle.example.com/saml"
entity_id = "https://spindle.example.com/saml/metadata"
cert_file = "/etc/spindle/saml-cert.pem"
key_file = "/etc/spindle/saml-key.pem"
```

### Auth Flow (sequence)

1. **Client** → `GET /v1/auth/login?connector=saml` on Spindle
2. **Spindle** generates SAML AuthnRequest and redirects to the IdP
3. **IdP** authenticates the user, sends SAML assertion back to Spindle's ACS URL
4. **Spindle** validates the assertion signature (using configured cert)
5. Extracts `subject` (NameID) and `groups` (attributes) from the assertion
6. JIT provisions the user (same as OIDC flow above)
7. Issues session JWTs

### Key Points

- Certificates are managed via `cert_file` / `key_file` paths (PEM format)
- Metadata is available at the entity ID URL for IdP configuration
- SAML assertion validation uses `spindle-saml` crate (quick-xml based parser)

---

## 4. LDAP

LDAP connector via Dex. Spindle delegates LDAP authentication to Dex, which
binds to the LDAP server and returns user attributes.

### Configuration

```toml
[identity.ldap]
host = "ldap.corp.example.com"
port = 389
use_tls = true
bind_dn = "cn=spindle,ou=services,dc=example,dc=com"
bind_password = "ldap-service-password"
user_search_base = "ou=users,dc=example,dc=com"
user_filter = "(uid={username})"
group_search_base = "ou=groups,dc=example,dc=com"
group_filter = "(member={dn})"
```

### Auth Flow (sequence)

1. **Client** → `GET /v1/auth/login?connector=ldap&subject=user@example.com&groups=engineers`
2. **Spindle** treats LDAP like any other connector — it doesn't bind to LDAP directly
3. **Dex** (external) performs the LDAP bind and returns user attributes to the client
4. The client passes `subject` and `groups` to Spindle's login endpoint
5. Spindle JIT-provisions the user with `connector=ldap`

### Key Points

- Spindle itself does NOT bind to LDAP — it trusts the connector/subject/groups
  passed to the login endpoint, which should come from a Dex-authenticated session
- The `spindle-dex` `ldap_connector.rs` module provides health-check connectivity
  testing to the LDAP server
- LDAP config is used to generate Dex YAML configuration

---

## 5. OIDC / Dex Integration

Dex is an external OIDC provider that federates multiple backends (SAML, LDAP,
GitHub, Google, etc.). Spindle trusts Dex as the sole IdP.

### Configuration

```toml
[identity]
issuer_url = "http://dex:5556"
client_id = "spindle"
client_secret = "CHANGE_ME"

[identity.oidc]
issuer = "http://dex:5556"
client_id = "spindle"
client_secret = "CHANGE_ME"
redirect_url = "http://spindle:3000/v1/auth/callback"
```

### Auth Flow (sequence)

1. **Client** → browser navigates to Dex login URL
2. **Dex** authenticates user via configured connector (GitHub, Google, SAML, LDAP...)
3. **Dex** issues an OIDC authorization code → redirected to Spindle
4. **Spindle** exchanges code for tokens with Dex
5. Extracts `subject` and `groups` from the ID token
6. JIT-provisions the user with `connector=oidc`
7. Issues Spindle session JWTs

### Health Checking

Spindle's `/health` endpoint probes Dex connectivity:

```
GET /health → subsystems[].name == "dex"
```

Uses `DexHealthChecker` which issues an HTTP GET to `issuer_url/.well-known/openid-configuration`.

---

## 6. JWKS (External JWT Validation)

Spindle can validate JWTs against an external JWKS (JSON Web Key Set) endpoint,
allowing tokens issued by other services to be accepted.

### Configuration

| Env Var | Description |
|---|---|
| `SPINDLE_JWKS_URL` | URL to fetch JWKS (e.g., `https://dex:5556/keys`) |

### How It Works

1. `jwk.rs` fetches the JWKS from the configured URL
2. When a JWT arrives, the key ID (`kid`) in the JWT header is matched against the JWKS
3. The JWT is validated using the matching public key
4. Valid tokens → role extracted from `scope` claim → injected into `X-User-Role` header

### Key Points

- JWKS is cached after first fetch; refresh on key rotation
- Works alongside the local HS256 JWT validation (both are tried)
- If `SPINDLE_JWKS_URL` is unset, JWKS validation is disabled

---

## Role Hierarchy

Roles are extracted from the JWT `scope` claim (comma-separated). The highest
privilege role wins:

| Role | Permissions |
|---|---|
| `admin` | All endpoints, dead-letter queue, waiver management |
| `token-admin` | Token management (not yet exposed as HTTP) |
| `compliance-auditor` | Read compliance + waivers, no writes |
| `ingest` | Ingest endpoints only |
| `viewer` | Read-only access to all query endpoints |

---

## Session Management

Sessions are managed via HS256 JWTs with configurable expiry:

- **Access token**: short-lived (default 1 hour)
- **Refresh token**: long-lived (default 24 hours)
- **Secret**: `SPINDLE_JWT_SECRET` env var (required in production)
- **Algorithm**: HS256

The `require_jwt_role` middleware validates the JWT, extracts the role, and
injects it into the `X-User-Role` header for downstream handlers.

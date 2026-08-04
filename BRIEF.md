# Sergey — Spindle M0-10: Dex Integration Setup

Requirement: ADR-05. Final M0 task.

## What to build
- Create `spindle-dex` crate
- Generate `dex.config.yaml` from Spindle config (figment, same pattern as spindle-config)
- Dex sidecar: `spindle-server` starts Dex as child process, or operator runs separately
- OIDC, SAML, LDAP connector stanzas in generated config — mapped from Spindle config sections
- Health check: poll Dex `/.well-known/openid-configuration` until ready, then proceed

## Tests
- Generate config from SpindleConfig → valid YAML
- Dex starts with generated config → discovery doc returns 200
- Missing required fields → clear error

## Verify
`cargo test -p spindle-dex` → green, then push. This closes M0.

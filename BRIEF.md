# Sergey — Spindle M0-09: Identity Model Interface

Requirement: IDP-01. **Traits only — no implementation.** This is the contract that C6/C7 build against. Freeze it here.

## What to build
- `spindle-identity::Identity` trait:
  - `authenticate(connector, credentials) -> Principal`
  - `resolve_groups(principal) -> Groups`
  - `map_claims(principal, rules) -> InternalRoles`
- `Principal` struct: `subject: String`, `source: ConnectorId`, `claims: HashMap`, `groups: Vec<String>`
- `InternalRoles` struct: roles + scopes
- `ConnectorId` newtype

## Tests
- Trait compiles
- No implementation — just the contract

## Verify
`cargo build -p spindle-identity` → compiles, push.

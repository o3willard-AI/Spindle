//! Identity model interface for Spindle.
//!
//! This crate defines the traits and types that C6/C7 build against.
//! No implementation — just the contract.

use std::collections::HashMap;

/// Unique identifier for an authentication connector (OAuth, SAML, API key, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectorId(pub u32);

/// The authenticated principal — who made a request and what they are.
#[derive(Debug, Clone)]
pub struct Principal {
    /// Subject identifier (user ID, email, etc.).
    pub subject: String,
    /// Source connector that authenticated this principal.
    pub source: ConnectorId,
    /// Arbitrary claims from the identity provider.
    pub claims: HashMap<String, String>,
    /// Groups/roles assigned to this principal.
    pub groups: Vec<String>,
}

/// Internal roles derived from principal claims and group membership.
#[derive(Debug, Clone)]
pub struct InternalRoles {
    /// Role names (e.g., "admin", "editor", "viewer").
    pub roles: Vec<String>,
    /// Scopes granted to this principal.
    pub scopes: Vec<String>,
}

/// Identity provider trait — the contract that connectors implement.
pub trait Identity {
    /// Authenticate a request and produce a Principal.
    fn authenticate(
        &self,
        credentials: &str,
    ) -> Result<Principal, String>;

    /// Resolve groups for a given principal.
    fn resolve_groups(&self, principal: &Principal) -> Result<Vec<String>, String>;

    /// Map claims to internal roles using identity rules.
    fn map_claims(&self, principal: &Principal, rules: &HashMap<String, String>) -> Result<InternalRoles, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connector_id() {
        let id = ConnectorId(1);
        assert_eq!(id, ConnectorId(1));
        assert_ne!(id, ConnectorId(2));
    }

    #[test]
    fn test_principal() {
        let principal = Principal {
            subject: "user@example.com".to_string(),
            source: ConnectorId(1),
            claims: HashMap::new(),
            groups: vec!["users".to_string()],
        };

        assert_eq!(principal.subject, "user@example.com");
        assert_eq!(principal.source, ConnectorId(1));
        assert_eq!(principal.groups.len(), 1);
    }

    #[test]
    fn test_internal_roles() {
        let roles = InternalRoles {
            roles: vec!["admin".to_string(), "editor".to_string()],
            scopes: vec!["read".to_string(), "write".to_string()],
        };

        assert_eq!(roles.roles.len(), 2);
        assert_eq!(roles.scopes.len(), 2);
    }

    #[test]
    fn test_identity_trait_compiles() {
        // This test just verifies the trait can be used
        struct DummyIdentity;

        impl Identity for DummyIdentity {
            fn authenticate(
                &self,
                _credentials: &str,
            ) -> Result<Principal, String> {
                Ok(Principal {
                    subject: "test".to_string(),
                    source: ConnectorId(0),
                    claims: HashMap::new(),
                    groups: vec![],
                })
            }

            fn resolve_groups(&self, _principal: &Principal) -> Result<Vec<String>, String> {
                Ok(vec!["default".to_string()])
            }

            fn map_claims(
                &self,
                _principal: &Principal,
                _rules: &HashMap<String, String>,
            ) -> Result<InternalRoles, String> {
                Ok(InternalRoles {
                    roles: vec!["default".to_string()],
                    scopes: vec!["default".to_string()],
                })
            }
        }

        let identity = DummyIdentity;
        let principal = identity.authenticate("test").unwrap();
        assert_eq!(principal.subject, "test");
    }
}

use spindle_identity::Identity;
use spindle_identity::Principal;
use spindle_identity::InternalRoles;
use spindle_identity::ConnectorId;

#[test]
fn test_identity_trait() {
    struct TestIdentity;

    impl Identity for TestIdentity {
        fn authenticate(&self, credentials: &str) -> Result<Principal, String> {
            if credentials == "valid" {
                Ok(Principal {
                    subject: "test@example.com".to_string(),
                    source: ConnectorId(1),
                    claims: std::collections::HashMap::new(),
                    groups: vec!["users".to_string()],
                })
            } else {
                Err("Invalid credentials".to_string())
            }
        }

        fn resolve_groups(&self, principal: &Principal) -> Result<Vec<String>, String> {
            Ok(principal.groups.clone())
        }

        fn map_claims(
            &self,
            principal: &Principal,
            rules: &std::collections::HashMap<String, String>,
        ) -> Result<InternalRoles, String> {
            Ok(InternalRoles {
                roles: vec!["default".to_string()],
                scopes: vec!["read".to_string()],
            })
        }
    }

    let identity = TestIdentity;
    let principal = identity.authenticate("valid").unwrap();
    assert_eq!(principal.subject, "test@example.com");

    let groups = identity.resolve_groups(&principal).unwrap();
    assert_eq!(groups, vec!["users"]);

    let roles = identity.map_claims(&principal, &std::collections::HashMap::new()).unwrap();
    assert_eq!(roles.roles, vec!["default"]);
}

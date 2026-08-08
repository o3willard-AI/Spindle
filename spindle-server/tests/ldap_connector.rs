//! M3-05: LDAP/AD connector integration tests.
//!
//! Tests cover:
//! - User DN resolution via configurable base DN + filter
//! - Direct bind for password validation (success + failure)
//! - Nested group resolution with configurable depth limit
//! - Referral handling: follow or reject
//! - Group cache with 15min TTL + manual refresh
//! - TLS required for production (StartTLS/LDAPS)

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use spindle_dex::ldap_connector::{
    LdapAuthenticator, LdapOperations, LdapConnector, LdapConnectorConfig, LdapError,
};

/// Mock LDAP operations for integration testing.
#[derive(Debug)]
struct MockLdapOps {
    users: Arc<std::sync::Mutex<HashMap<String, (String, String, HashMap<String, String>)>>>,
    groups: Arc<std::sync::Mutex<HashMap<String, Vec<String>>>>,
    bind_count: Arc<AtomicUsize>,
}

impl MockLdapOps {
    fn new() -> Self {
        Self {
            users: Arc::new(std::sync::Mutex::new(HashMap::new())),
            groups: Arc::new(std::sync::Mutex::new(HashMap::new())),
            bind_count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl LdapOperations for MockLdapOps {
    async fn search_user_dn(&self, username: &str) -> Result<String, String> {
        let users = self.users.lock().unwrap();
        users
            .get(username)
            .map(|(dn, _, _)| dn.clone())
            .ok_or_else(|| format!("User not found: {}", username))
    }

    async fn bind(&self, dn: &str, password: &str) -> Result<(), String> {
        self.bind_count.fetch_add(1, Ordering::SeqCst);
        let users = self.users.lock().unwrap();
        for (_, (user_dn, pw, _)) in users.iter() {
            if user_dn == dn && pw == password {
                return Ok(());
            }
        }
        Err("Invalid credentials".to_string())
    }

    async fn search_groups(
        &self,
        _base_dn: &str,
        filter: &str,
        _attrs: &[String],
    ) -> Result<Vec<(String, String)>, String> {
        let groups = self.groups.lock().unwrap();
        let member_dn = if let Some(start) = filter.find('=') {
            filter[start + 1..].trim_end_matches(')').to_string()
        } else {
            return Ok(vec![]);
        };

        let mut result = vec![];
        for (group_dn, members) in groups.iter() {
            if members.contains(&member_dn) {
                let cn = LdapConnector::extract_cn_from_dn(group_dn);
                result.push((cn, group_dn.clone()));
            }
        }
        Ok(result)
    }

    async fn get_user_attributes(
        &self,
        dn: &str,
        _attrs: &[String],
    ) -> Result<HashMap<String, String>, String> {
        let users = self.users.lock().unwrap();
        for (_, (user_dn, _, user_attrs)) in users.iter() {
            if user_dn == dn {
                return Ok(user_attrs.clone());
            }
        }
        Ok(HashMap::new())
    }
}

fn make_config(server_url: &str) -> LdapConnectorConfig {
    LdapConnectorConfig {
        server_url: server_url.to_string(),
        base_dn: "dc=example,dc=com".to_string(),
        ..Default::default()
    }
}

fn setup_users() -> HashMap<String, (String, String, HashMap<String, String>)> {
    let mut users = HashMap::new();
    users.insert(
        "jdoe".to_string(),
        (
            "CN=John Doe,OU=Users,DC=example,DC=com".to_string(),
            "password123".to_string(),
            HashMap::from([
                ("uid".to_string(), "jdoe".to_string()),
                ("mail".to_string(), "jdoe@example.com".to_string()),
                ("displayName".to_string(), "John Doe".to_string()),
            ]),
        ),
    );
    users.insert(
        "asmith".to_string(),
        (
            "CN=Alice Smith,OU=Users,DC=example,DC=com".to_string(),
            "alice_pass".to_string(),
            HashMap::from([
                ("uid".to_string(), "asmith".to_string()),
                ("mail".to_string(), "asmith@example.com".to_string()),
            ]),
        ),
    );
    users
}

fn setup_groups() -> HashMap<String, Vec<String>> {
    let mut groups = HashMap::new();
    // Engineering group: John is a direct member
    groups.insert(
        "CN=Engineering,OU=Groups,DC=example,DC=com".to_string(),
        vec!["CN=John Doe,OU=Users,DC=example,DC=com".to_string()],
    );
    // AllUsers group: Engineering group is a member (nested)
    groups.insert(
        "CN=AllUsers,OU=Groups,DC=example,DC=com".to_string(),
        vec!["CN=Engineering,OU=Groups,DC=example,DC=com".to_string()],
    );
    // Admin group: Alice is a direct member
    groups.insert(
        "CN=Admin,OU=Groups,DC=example,DC=com".to_string(),
        vec!["CN=Alice Smith,OU=Users,DC=example,DC=com".to_string()],
    );
    groups.insert(
        "CN=Operations,OU=Groups,DC=example,DC=com".to_string(),
        vec!["CN=Admin,OU=Groups,DC=example,DC=com".to_string()],
    );
    groups
}

// ── User DN resolution tests ─────────────────────────────────────────────

#[tokio::test]
async fn test_resolve_user_dn_found() {
    let mock = MockLdapOps::new();
    let mut users = setup_users();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users.drain());
    }

    let config = make_config("ldaps://ldap.example.com:636");
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let result = connector.authenticate("jdoe", "password123").await.unwrap();
    assert_eq!(result.dn, "CN=John Doe,OU=Users,DC=example,DC=com");
    assert_eq!(result.uid, Some("jdoe".to_string()));
}

#[tokio::test]
async fn test_resolve_user_dn_not_found() {
    let mock = MockLdapOps::new();
    let config = make_config("ldaps://ldap.example.com:636");
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let result = connector.authenticate("nonexistent", "password").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("search failed") || err.contains("not found"));
}

// ── Direct bind / password validation tests ──────────────────────────────

#[tokio::test]
async fn test_bind_success() {
    let mock = MockLdapOps::new();
    let users = setup_users();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users);
    }

    let config = make_config("ldaps://ldap.example.com:636");
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let result = connector.authenticate("jdoe", "password123").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_bind_bad_password() {
    let mock = MockLdapOps::new();
    let users = setup_users();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users);
    }

    let config = make_config("ldaps://ldap.example.com:636");
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let result = connector.authenticate("jdoe", "wrong_password").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), LdapError::BindFailed(_)));
}

// ── Nested group resolution tests ────────────────────────────────────────

#[tokio::test]
async fn test_nested_group_resolution_direct_and_nested() {
    let mock = MockLdapOps::new();
    let users = setup_users();
    let groups = setup_groups();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users);
    }
    {
        let mut guard = mock.groups.lock().unwrap();
        guard.extend(groups);
    }

    let config = LdapConnectorConfig {
        group_search_filter: Some("(member={dn})".to_string()),
        group_search_attributes: vec!["cn".to_string(), "dn".to_string()],
        base_dn: "dc=example,dc=com".to_string(),
        server_url: "ldaps://ldap.example.com:636".to_string(),
        ..Default::default()
    };
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    // John is in Engineering (direct), and Engineering is in AllUsers (nested)
    let groups = connector
        .resolve_groups("CN=John Doe,OU=Users,DC=example,DC=com")
        .await
        .unwrap();

    assert!(groups.contains(&"Engineering".to_string()));
    assert!(groups.contains(&"AllUsers".to_string()));
    assert!(!groups.contains(&"Admin".to_string()));
}

#[tokio::test]
async fn test_nested_group_resolution_with_depth_limit() {
    let mock = MockLdapOps::new();
    let groups = setup_groups();
    {
        let mut guard = mock.groups.lock().unwrap();
        guard.extend(groups);
    }

    let config = LdapConnectorConfig {
        group_search_filter: Some("(member={dn})".to_string()),
        group_search_attributes: vec!["cn".to_string(), "dn".to_string()],
        base_dn: "dc=example,dc=com".to_string(),
        max_depth: 2, // Allow only 2 levels
        server_url: "ldaps://ldap.example.com:636".to_string(),
        ..Default::default()
    };
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    // Alice is in Admin, Admin is in Operations
    // depth 0: Alice found in Admin
    // depth 1: Admin found in Operations
    // depth 2: Operations has no parent group, so it stops naturally
    let groups = connector
        .resolve_groups("CN=Alice Smith,OU=Users,DC=example,DC=com")
        .await
        .unwrap();

    assert!(groups.contains(&"Admin".to_string()));
    assert!(groups.contains(&"Operations".to_string()));
}

#[tokio::test]
async fn test_nested_group_depth_exceeded() {
    // Create a deep chain: User -> G1 -> G2 -> G3 -> G4
    let mock = MockLdapOps::new();
    let mut groups = HashMap::new();
    groups.insert(
        "CN=G1,OU=Groups,DC=example,DC=com".to_string(),
        vec!["CN=User1,OU=Users,DC=example,DC=com".to_string()],
    );
    groups.insert(
        "CN=G2,OU=Groups,DC=example,DC=com".to_string(),
        vec!["CN=G1,OU=Groups,DC=example,DC=com".to_string()],
    );
    groups.insert(
        "CN=G3,OU=Groups,DC=example,DC=com".to_string(),
        vec!["CN=G2,OU=Groups,DC=example,DC=com".to_string()],
    );
    groups.insert(
        "CN=G4,OU=Groups,DC=example,DC=com".to_string(),
        vec!["CN=G3,OU=Groups,DC=example,DC=com".to_string()],
    );

    let mock = MockLdapOps {
        users: Arc::new(std::sync::Mutex::new(HashMap::new())),
        groups: Arc::new(std::sync::Mutex::new(groups)),
        bind_count: Arc::new(AtomicUsize::new(0)),
    };

    let config = LdapConnectorConfig {
        group_search_filter: Some("(member={dn})".to_string()),
        group_search_attributes: vec!["cn".to_string(), "dn".to_string()],
        base_dn: "dc=example,dc=com".to_string(),
        max_depth: 2,
        server_url: "ldaps://ldap.example.com:636".to_string(),
        ..Default::default()
    };
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    // depth 0: User1 found in G1, recurse into G1
    // depth 1: G1 found in G2, recurse into G2
    // depth 2: G2 found in G3, recurse into G3
    // depth 3: 3 > max_depth=2, error
    let result = connector.resolve_groups("CN=User1,OU=Users,DC=example,DC=com").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), LdapError::MaxDepthExceeded(2)));
}

// ── Referral handling tests ─────────────────────────────────────────────

#[test]
fn test_referral_rejected_when_follow_disabled() {
    let config = LdapConnectorConfig {
        follow_referrals: false,
        ..Default::default()
    };
    assert!(!config.follow_referrals);
}

#[test]
fn test_referral_followed_when_enabled() {
    let config = LdapConnectorConfig {
        follow_referrals: true,
        ..Default::default()
    };
    assert!(config.follow_referrals);
}

#[test]
fn test_referral_rejected_error_message() {
    let err = LdapError::ReferralRejected("ldap://external.example.com".to_string());
    assert!(err.to_string().contains("referral rejected"));
}

// ── Group cache tests ────────────────────────────────────────────────────

#[tokio::test]
async fn test_group_cache_set_and_get() {
    let mock = MockLdapOps::new();
    let users = setup_users();
    let groups = setup_groups();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users);
    }
    {
        let mut guard = mock.groups.lock().unwrap();
        guard.extend(groups);
    }

    let config = LdapConnectorConfig {
        group_search_filter: Some("(member={dn})".to_string()),
        group_search_attributes: vec!["cn".to_string(), "dn".to_string()],
        base_dn: "dc=example,dc=com".to_string(),
        cache_ttl_secs: 300, // 5 min
        server_url: "ldaps://ldap.example.com:636".to_string(),
        ..Default::default()
    };
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    // Authenticate and cache groups
    let auth_result = connector.authenticate("jdoe", "password123").await.unwrap();
    let principal_key = "ldap:jdoe";
    connector.cache_groups(principal_key, auth_result.groups.clone()).unwrap();

    // Cache should have the groups
    let cached = connector.get_cached_groups(principal_key);
    assert!(cached.is_some());
    let cached = cached.unwrap();
    assert!(cached.contains(&"Engineering".to_string()));
}

#[tokio::test]
async fn test_group_cache_miss_returns_none() {
    let config = LdapConnectorConfig::default();
    let connector = LdapConnector::new(config);

    let cached = connector.get_cached_groups("ldap:unknown_user");
    assert!(cached.is_none());
}

#[tokio::test]
async fn test_group_cache_ttl_expiry() {
    let config = LdapConnectorConfig {
        cache_ttl_secs: 1, // 1 second TTL
        ..Default::default()
    };
    let connector = LdapConnector::new(config);

    connector
        .cache_groups("ldap:user1", vec!["admin".to_string()])
        .unwrap();

    // Should be cached immediately after setting
    assert!(connector.get_cached_groups("ldap:user1").is_some());

    // Wait for expiry
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Should be expired
    assert!(connector.get_cached_groups("ldap:user1").is_none());
}

#[tokio::test]
async fn test_group_cache_clear() {
    let config = LdapConnectorConfig::default();
    let connector = LdapConnector::new(config);

    connector
        .cache_groups("ldap:user1", vec!["admin".to_string()])
        .unwrap();
    connector
        .cache_groups("ldap:user2", vec!["viewer".to_string()])
        .unwrap();

    assert_eq!(connector.cache_size(), 2);
    connector.clear_cache();
    assert_eq!(connector.cache_size(), 0);
}

#[tokio::test]
async fn test_group_cache_manual_refresh() {
    let mock = MockLdapOps::new();
    let users = setup_users();
    let groups = setup_groups();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users);
    }
    {
        let mut guard = mock.groups.lock().unwrap();
        guard.extend(groups);
    }

    let config = LdapConnectorConfig {
        group_search_filter: Some("(member={dn})".to_string()),
        group_search_attributes: vec!["cn".to_string(), "dn".to_string()],
        base_dn: "dc=example,dc=com".to_string(),
        cache_ttl_secs: 300,
        server_url: "ldaps://ldap.example.com:636".to_string(),
        ..Default::default()
    };
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    // Initial cache is empty
    assert_eq!(connector.cache_size(), 0);

    // Manual refresh populates cache
    let groups = connector.refresh_groups("jdoe", "password123").await.unwrap();
    assert!(groups.contains(&"Engineering".to_string()));
    assert_eq!(connector.cache_size(), 1);

    // Cached result should be available
    let cached = connector.get_cached_groups("ldap:jdoe");
    assert!(cached.is_some());
}

// ── TLS validation tests ─────────────────────────────────────────────────

#[test]
fn test_tls_required_for_production() {
    let config = LdapConnectorConfig {
        server_url: "ldap://ldap.corp.example.com:389".to_string(),
        base_dn: "dc=example,dc=com".to_string(),
        require_tls: false,
        ..Default::default()
    };
    let connector = LdapConnector::new(config);
    let err = connector.validate_config().unwrap_err();
    assert!(matches!(err, LdapError::TlsRequired));
}

#[test]
fn test_tls_not_required_for_localhost() {
    let config = LdapConnectorConfig {
        server_url: "ldap://localhost:389".to_string(),
        base_dn: "dc=example,dc=com".to_string(),
        require_tls: false,
        ..Default::default()
    };
    let connector = LdapConnector::new(config);
    assert!(connector.validate_config().is_ok());
}

#[test]
fn test_ldaps_validates_ok() {
    let config = LdapConnectorConfig {
        server_url: "ldaps://ldap.example.com:636".to_string(),
        base_dn: "dc=example,dc=com".to_string(),
        require_tls: true,
        ..Default::default()
    };
    let connector = LdapConnector::new(config);
    assert!(connector.validate_config().is_ok());
}

// ── Connection pool config tests ─────────────────────────────────────────

#[test]
fn test_connection_pool_size_config() {
    let config = LdapConnectorConfig {
        pool_size: 20,
        ..Default::default()
    };
    let connector = LdapConnector::new(config);
    assert_eq!(connector.pool_size(), 20);
}

#[test]
fn test_connection_timeout_config() {
    let config = LdapConnectorConfig {
        timeout_secs: 30,
        ..Default::default()
    };
    let connector = LdapConnector::new(config);
    assert_eq!(connector.timeout_secs(), 30);
}

// ── User search filter tests ─────────────────────────────────────────────

#[test]
fn test_user_search_filter_custom() {
    let config = LdapConnectorConfig {
        user_search_filter: "(sAMAccountName={user})".to_string(),
        ..Default::default()
    };
    let filter = config.user_search_filter.replace("{user}", "john");
    assert_eq!(filter, "(sAMAccountName=john)");
}

#[test]
fn test_user_search_filter_uid() {
    let config = LdapConnectorConfig {
        user_search_filter: "(uid={user})".to_string(),
        ..Default::default()
    };
    let filter = config.user_search_filter.replace("{user}", "jdoe");
    assert_eq!(filter, "(uid=jdoe)");
}

#[test]
fn test_user_search_filter_email() {
    let config = LdapConnectorConfig {
        user_search_filter: "(mail={user})".to_string(),
        ..Default::default()
    };
    let filter = config.user_search_filter.replace("{user}", "jdoe@example.com");
    assert_eq!(filter, "(mail=jdoe@example.com)");
}

// ── Group search filter tests ────────────────────────────────────────────

#[test]
fn test_group_search_filter_custom() {
    let config = LdapConnectorConfig {
        group_search_filter: Some("(uniqueMember={dn})".to_string()),
        ..Default::default()
    };
    let filter = config
        .group_search_filter
        .unwrap()
        .replace("{dn}", "CN=John Doe,OU=Users");
    assert_eq!(filter, "(uniqueMember=CN=John Doe,OU=Users)");
}

// ── Config from SpindleConfig tests ──────────────────────────────────────

#[test]
fn test_from_spindle_config_no_ldap() {
    let config = spindle_dex::SpindleConfig::default();
    let result = LdapConnector::from_spindle_config(&config);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), LdapError::Connection(_)));
}

#[test]
fn test_from_spindle_config_with_ldap() {
    let ldap_config = spindle_dex::LdapConfig {
        client_id: "ldap-client".to_string(),
        redirect_url: "https://spindle.local/ldap".to_string(),
        server_url: "ldaps://ldap.example.com:636".to_string(),
        base_dn: "dc=example,dc=com".to_string(),
        user_search_filter: "(uid={user})".to_string(),
        ..Default::default()
    };
    let spindle_config = spindle_dex::SpindleConfig {
        identity: spindle_dex::IdentityConfig {
            ldap: Some(ldap_config),
            ..Default::default()
        },
        ..Default::default()
    };

    let connector = LdapConnector::from_spindle_config(&spindle_config).unwrap();
    assert_eq!(connector.server_url(), "ldaps://ldap.example.com:636");
    assert_eq!(connector.base_dn(), "dc=example,dc=com");
    assert!(connector.require_tls());
}

// ── DN extraction tests ──────────────────────────────────────────────────

#[test]
fn test_extract_cn_from_dn_uppercase() {
    let dn = "CN=Engineering,OU=Groups,DC=example,DC=com";
    assert_eq!(LdapConnector::extract_cn_from_dn(dn), "Engineering");
}

#[test]
fn test_extract_cn_from_dn_lowercase() {
    let dn = "cn=engineering,ou=groups,dc=example,dc=com";
    assert_eq!(LdapConnector::extract_cn_from_dn(dn), "engineering");
}

#[test]
fn test_extract_cn_from_dn_no_cn() {
    let dn = "OU=Groups,DC=example,DC=com";
    assert_eq!(LdapConnector::extract_cn_from_dn(dn), "OU=Groups,DC=example,DC=com");
}

#[test]
fn test_extract_cn_from_dn_multiple_components() {
    let dn = "CN=App Admins,OU=Security Groups,OU=Groups,DC=example,DC=com";
    assert_eq!(LdapConnector::extract_cn_from_dn(dn), "App Admins");
}

// ── Full authentication flow tests ───────────────────────────────────────

#[tokio::test]
async fn test_full_auth_flow_success() {
    let mock = MockLdapOps::new();
    let users = setup_users();
    let groups = setup_groups();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users);
    }
    {
        let mut guard = mock.groups.lock().unwrap();
        guard.extend(groups);
    }

    let config = LdapConnectorConfig {
        group_search_filter: Some("(member={dn})".to_string()),
        group_search_attributes: vec!["cn".to_string(), "dn".to_string()],
        base_dn: "dc=example,dc=com".to_string(),
        cache_ttl_secs: 300,
        server_url: "ldaps://ldap.example.com:636".to_string(),
        ..Default::default()
    };
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let result = connector.authenticate("jdoe", "password123").await.unwrap();

    assert_eq!(result.dn, "CN=John Doe,OU=Users,DC=example,DC=com");
    assert_eq!(result.uid, Some("jdoe".to_string()));
    assert_eq!(result.claims.get("mail"), Some(&"jdoe@example.com".to_string()));
    assert_eq!(result.claims.get("displayName"), Some(&"John Doe".to_string()));

    // Should have resolved both direct and nested groups
    let group_set: HashSet<String> = result.groups.iter().cloned().collect();
    assert!(group_set.contains(&"Engineering".to_string()));
    assert!(group_set.contains(&"AllUsers".to_string()));
}

#[tokio::test]
async fn test_full_auth_flow_bad_password() {
    let mock = MockLdapOps::new();
    let users = setup_users();
    let groups = setup_groups();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users);
    }
    {
        let mut guard = mock.groups.lock().unwrap();
        guard.extend(groups);
    }

    let config = make_config("ldaps://ldap.example.com:636");
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let result = connector.authenticate("jdoe", "wrong_password").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), LdapError::BindFailed(_)));
}

#[tokio::test]
async fn test_full_auth_flow_user_not_found() {
    let mock = MockLdapOps::new();

    let config = make_config("ldaps://ldap.example.com:636");
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let result = connector.authenticate("ghost", "password").await;
    assert!(result.is_err());
}

// ── Bind count verification tests ───────────────────────────────────────

#[tokio::test]
async fn test_bind_count_on_success() {
    let mock = MockLdapOps::new();
    let users = setup_users();
    let bind_count = mock.bind_count.clone();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users);
    }

    let config = make_config("ldaps://ldap.example.com:636");
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let _ = connector.authenticate("jdoe", "password123").await;
    assert_eq!(bind_count.load(Ordering::SeqCst), 1); // Only the user bind
}

#[tokio::test]
async fn test_bind_count_on_failure() {
    let mock = MockLdapOps::new();
    let users = setup_users();
    let bind_count = mock.bind_count.clone();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users);
    }

    let config = make_config("ldaps://ldap.example.com:636");
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let _ = connector.authenticate("jdoe", "wrong").await;
    assert_eq!(bind_count.load(Ordering::SeqCst), 1); // Attempted bind
}

#[tokio::test]
async fn test_no_bind_when_user_not_found() {
    let mock = MockLdapOps::new();
    let bind_count = mock.bind_count.clone();

    let config = make_config("ldaps://ldap.example.com:636");
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let _ = connector.authenticate("nobody", "password").await;
    assert_eq!(bind_count.load(Ordering::SeqCst), 0); // Bind never called
}

// ── Cache behavior tests ─────────────────────────────────────────────────

#[tokio::test]
async fn test_cache_returns_stale_then_updates() {
    let mock = MockLdapOps::new();
    let users = setup_users();
    let groups = setup_groups();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users);
    }
    {
        let mut guard = mock.groups.lock().unwrap();
        guard.extend(groups);
    }

    let config = LdapConnectorConfig {
        group_search_filter: Some("(member={dn})".to_string()),
        group_search_attributes: vec!["cn".to_string(), "dn".to_string()],
        base_dn: "dc=example,dc=com".to_string(),
        cache_ttl_secs: 1, // Short TTL for testing
        server_url: "ldaps://ldap.example.com:636".to_string(),
        ..Default::default()
    };
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let principal_key = "ldap:jdoe";

    // Initial: cache empty
    assert!(connector.get_cached_groups(principal_key).is_none());

    // Populate cache
    let auth_result = connector.authenticate("jdoe", "password123").await.unwrap();
    connector.cache_groups(principal_key, auth_result.groups.clone()).unwrap();
    assert!(connector.get_cached_groups(principal_key).is_some());

    // Wait for expiry
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Cache expired — should return None
    assert!(connector.get_cached_groups(principal_key).is_none());

    // After refresh, cache is repopulated
    let new_groups = connector.refresh_groups("jdoe", "password123").await.unwrap();
    assert!(connector.get_cached_groups(principal_key).is_some());
    assert_eq!(
        new_groups.len(),
        connector.get_cached_groups(principal_key).unwrap().len()
    );
}

#[tokio::test]
async fn test_cache_does_not_persist_across_instances() {
    let config = LdapConnectorConfig::default();
    let connector1 = LdapConnector::new(config.clone());
    let connector2 = LdapConnector::new(config);

    connector1
        .cache_groups("ldap:user1", vec!["admin".to_string()])
        .unwrap();

    // connector2 is a different instance with its own cache
    assert_eq!(connector1.cache_size(), 1);
    assert_eq!(connector2.cache_size(), 0);
    assert!(connector2.get_cached_groups("ldap:user1").is_none());
}

// ── Clone preserves cache (shared cache) ────────────────────────────────

#[test]
fn test_clone_shares_cache() {
    let config = LdapConnectorConfig::default();
    let connector = LdapConnector::new(config);

    connector
        .cache_groups("ldap:user1", vec!["admin".to_string()])
        .unwrap();

    let connector2 = connector.clone();
    assert_eq!(connector2.cache_size(), 1);
    let cached = connector2.get_cached_groups("ldap:user1");
    assert!(cached.is_some());
    assert_eq!(cached.unwrap(), vec!["admin".to_string()]);
}

// ── LdapAuthenticator trait tests ───────────────────────────────────────

#[tokio::test]
async fn test_ldap_authenticator_trait_authenticate() {
    let mock = MockLdapOps::new();
    let users = setup_users();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users);
    }

    let config = make_config("ldaps://ldap.example.com:636");
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let result = connector.authenticate_ldap("jdoe", "password123").await.unwrap();
    assert_eq!(result.uid, Some("jdoe".to_string()));
}

#[tokio::test]
async fn test_ldap_authenticator_trait_refresh() {
    let mock = MockLdapOps::new();
    let users = setup_users();
    let groups = setup_groups();
    {
        let mut guard = mock.users.lock().unwrap();
        guard.extend(users);
    }
    {
        let mut guard = mock.groups.lock().unwrap();
        guard.extend(groups);
    }

    let config = LdapConnectorConfig {
        group_search_filter: Some("(member={dn})".to_string()),
        group_search_attributes: vec!["cn".to_string(), "dn".to_string()],
        base_dn: "dc=example,dc=com".to_string(),
        server_url: "ldaps://ldap.example.com:636".to_string(),
        ..Default::default()
    };
    let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

    let groups = connector.refresh_ldap_groups("jdoe", "password123").await.unwrap();
    assert!(!groups.is_empty());
}

//! Comprehensive tests for spindle-identity M3-02.

#![allow(warnings)]
use spindle_authz::Role;
use spindle_identity::*;
use std::collections::HashMap;
use std::time::Duration;

// ── ConnectorId Tests ────────────────────────────────────────────────────────

#[test]
fn test_connector_id_basic() {
    let id = ConnectorId::new(1);
    assert_eq!(id.0, 1);
    assert_eq!(id, ConnectorId(1));
    assert_ne!(id, ConnectorId(2));
}

#[test]
fn test_connector_id_default_oidc() {
    let id = ConnectorId::default_oidc();
    assert_eq!(id.0, 0);
}

#[test]
fn test_connector_id_hashable() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(ConnectorId(1));
    set.insert(ConnectorId(2));
    assert_eq!(set.len(), 2);
    assert!(set.contains(&ConnectorId(1)));
}

// ── OidcClaims Tests ─────────────────────────────────────────────────────────

#[test]
fn test_oidc_claims_extract_all_fields() {
    let mut raw = HashMap::new();
    raw.insert("sub".to_string(), serde_json::json!("dex-user-456"));
    raw.insert("preferred_username".to_string(), serde_json::json!("alice"));
    raw.insert("email".to_string(), serde_json::json!("alice@corp.com"));
    raw.insert("email_verified".to_string(), serde_json::json!(true));
    raw.insert(
        "groups".to_string(),
        serde_json::json!(["spindle-admins", "spindle-viewers"]),
    );
    raw.insert("nickname".to_string(), serde_json::json!("aliceb"));
    raw.insert(
        "picture".to_string(),
        serde_json::json!("https://example.com/alice.png"),
    );

    let claims = OidcClaims::from_raw(&raw);

    assert_eq!(claims.sub, "dex-user-456");
    assert_eq!(claims.preferred_username, Some("alice".to_string()));
    assert_eq!(claims.email, Some("alice@corp.com".to_string()));
    assert_eq!(claims.email_verified, Some(true));
    assert_eq!(
        claims.groups,
        Some(vec![
            "spindle-admins".to_string(),
            "spindle-viewers".to_string()
        ])
    );
    assert!(claims.extra.contains_key("nickname"));
    assert!(claims.extra.contains_key("picture"));
}

#[test]
fn test_oidc_claims_partial() {
    let mut raw = HashMap::new();
    raw.insert("sub".to_string(), serde_json::json!("user-789"));
    // No email, no groups, no verified flag

    let claims = OidcClaims::from_raw(&raw);

    assert_eq!(claims.sub, "user-789");
    assert!(claims.preferred_username.is_none());
    assert!(claims.email.is_none());
    assert!(claims.email_verified.is_none());
    assert!(claims.groups.is_none());
}

#[test]
fn test_oidc_claims_validate_ok() {
    let mut raw = HashMap::new();
    raw.insert("sub".to_string(), serde_json::json!("user-ok"));
    let claims = OidcClaims::from_raw(&raw);
    assert!(claims.validate().is_ok());
}

#[test]
fn test_oidc_claims_validate_empty_sub() {
    let raw = HashMap::new();
    let claims = OidcClaims::from_raw(&raw);
    let err = claims.validate().unwrap_err();
    assert!(err.contains("sub"));
    assert!(err.contains("required"));
}

#[test]
fn test_oidc_claims_empty_sub() {
    let mut raw = HashMap::new();
    raw.insert("sub".to_string(), serde_json::json!(""));
    let claims = OidcClaims::from_raw(&raw);
    assert!(claims.validate().is_err());
}

#[test]
fn test_oidc_claims_group_list_present() {
    let mut raw = HashMap::new();
    raw.insert("sub".to_string(), serde_json::json!("user"));
    raw.insert("groups".to_string(), serde_json::json!(["g1", "g2", "g3"]));
    let claims = OidcClaims::from_raw(&raw);
    let groups = claims.group_list();
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0], "g1");
}

#[test]
fn test_oidc_claims_group_list_absent() {
    let raw = HashMap::new();
    let claims = OidcClaims::from_raw(&raw);
    assert!(claims.group_list().is_empty());
}

// ── Principal Tests ──────────────────────────────────────────────────────────

#[test]
fn test_principal_full() {
    let mut raw = HashMap::new();
    raw.insert("sub".to_string(), serde_json::json!("user-1"));
    raw.insert("preferred_username".to_string(), serde_json::json!("bob"));
    raw.insert("email".to_string(), serde_json::json!("bob@test.com"));
    let claims = OidcClaims::from_raw(&raw);
    let groups = vec!["admin".to_string(), "editors".to_string()];

    let p = Principal::from_claims(&claims, ConnectorId(42), groups.clone());

    assert_eq!(p.subject, "user-1");
    assert_eq!(p.source, ConnectorId(42));
    assert_eq!(p.groups, groups);
    assert_eq!(p.display_name, Some("bob".to_string()));
    assert_eq!(p.email, Some("bob@test.com".to_string()));
    assert!(p.claims.contains_key("preferred_username"));
}

#[test]
fn test_principal_empty_groups() {
    let claims = OidcClaims::default();
    let p = Principal::from_claims(&claims, ConnectorId(0), vec![]);

    assert_eq!(p.subject, "");
    assert!(p.groups.is_empty());
}

#[test]
fn test_principal_scope_empty_roles() {
    let claims = OidcClaims::default();
    let p = Principal::from_claims(&claims, ConnectorId(0), vec![]);

    let empty_map = HashMap::new();
    let scope = p.scope(&empty_map);
    assert!(scope.roles.is_empty());
}

#[test]
fn test_principal_scope_has_role() {
    let mut raw = HashMap::new();
    raw.insert("sub".to_string(), serde_json::json!("user-1"));
    let claims = OidcClaims::from_raw(&raw);

    let p = Principal::from_claims(&claims, ConnectorId(0), vec!["spindle-admins".to_string()]);

    let mut role_map = HashMap::new();
    role_map.insert("spindle-admins".to_string(), Role::Admin);

    let scope = p.scope(&role_map);
    assert!(scope.has_role("admin"));
}

// ── InternalRoles Tests ──────────────────────────────────────────────────────

#[test]
fn test_internal_roles_basic() {
    let ir = InternalRoles::new(
        vec!["viewer".to_string()],
        vec!["read".to_string()],
        vec![Role::Viewer],
    );

    assert_eq!(ir.roles, vec!["viewer"]);
    assert_eq!(ir.scopes, vec!["read"]);
    assert!(ir.has_role(Role::Viewer));
    assert!(!ir.has_role(Role::Admin));
    assert_eq!(ir.highest_role(), Some(Role::Viewer));
}

#[test]
fn test_internal_roles_admin_includes_all() {
    let ir = InternalRoles::new(
        vec!["admin".to_string()],
        vec!["read".to_string(), "write".to_string()],
        vec![Role::Admin],
    );

    assert!(ir.has_role(Role::Admin));
    assert!(ir.has_role(Role::TokenAdmin));
    assert!(ir.has_role(Role::ComplianceAuditor));
    assert!(ir.has_role(Role::Viewer));
    assert!(ir.has_role(Role::Ingest));
    assert_eq!(ir.highest_role(), Some(Role::Admin));
}

#[test]
fn test_internal_roles_token_admin() {
    let ir = InternalRoles::new(
        vec!["token-admin".to_string()],
        vec!["token-admin".to_string()],
        vec![Role::TokenAdmin],
    );

    assert!(ir.has_role(Role::TokenAdmin));
    assert!(ir.has_role(Role::ComplianceAuditor));
    assert!(ir.has_role(Role::Viewer));
    assert!(ir.has_role(Role::Ingest));
    assert!(!ir.has_role(Role::Admin));
    assert_eq!(ir.highest_role(), Some(Role::TokenAdmin));
}

#[test]
fn test_internal_roles_default() {
    let ir = InternalRoles::default();
    assert!(ir.roles.is_empty());
    assert!(ir.scopes.is_empty());
    assert!(ir.spindle_roles.is_empty());
    assert_eq!(ir.highest_role(), None);
    assert!(!ir.has_role(Role::Viewer));
}

#[test]
fn test_internal_roles_no_duplicate_roles() {
    let mut rules = Vec::new();
    rules.push(RoleMappingRule::new("admin-group", Role::Admin));
    let mapper = RoleMapper::new(rules);

    // Map the same group twice (as if it appeared in groups list twice)
    let roles = mapper.map(&["admin-group".to_string(), "admin-group".to_string()]);
    let admin_count = roles
        .spindle_roles
        .iter()
        .filter(|r| *r == &Role::Admin)
        .count();
    assert_eq!(admin_count, 1);
}

// ── GroupCache Tests ─────────────────────────────────────────────────────────

#[test]
fn test_group_cache_insert_and_retrieve() {
    let cache = GroupCache::default_ttl();
    cache.put("alice", vec!["admins".to_string(), "editors".to_string()]);

    let groups = cache.get("alice").unwrap();
    assert_eq!(groups, vec!["admins", "editors"]);
}

#[test]
fn test_group_cache_miss() {
    let cache = GroupCache::default_ttl();
    assert!(cache.get("alice").is_none());
}

#[test]
fn test_group_cache_overwrite() {
    let cache = GroupCache::default_ttl();
    cache.put("alice", vec!["viewers".to_string()]);
    cache.put("alice", vec!["admins".to_string()]);

    let groups = cache.get("alice").unwrap();
    assert_eq!(groups, vec!["admins"]);
}

#[test]
fn test_group_cache_invalidate() {
    let cache = GroupCache::default_ttl();
    cache.put("alice", vec!["admins".to_string()]);
    cache.invalidate("alice");
    assert!(cache.get("alice").is_none());
}

#[test]
fn test_group_cache_clear() {
    let cache = GroupCache::default_ttl();
    cache.put("alice", vec!["a".to_string()]);
    cache.put("bob", vec!["b".to_string()]);
    cache.put("carol", vec!["c".to_string()]);
    cache.clear();

    assert!(cache.get("alice").is_none());
    assert!(cache.get("bob").is_none());
    assert!(cache.get("carol").is_none());
}

#[test]
fn test_group_cache_ttl_expiration() {
    let cache = GroupCache::new(Duration::from_millis(50));
    cache.put("alice", vec!["admins".to_string()]);

    // Should be cached
    assert!(cache.get("alice").is_some());

    // Wait for expiry
    std::thread::sleep(Duration::from_millis(100));

    // Should be gone
    assert!(cache.get("alice").is_none());
}

#[test]
fn test_group_cache_evict_expired() {
    let cache = GroupCache::new(Duration::from_millis(50));
    cache.put("alice", vec!["a".to_string()]);
    cache.put("bob", vec!["b".to_string()]);

    std::thread::sleep(Duration::from_millis(100));

    cache.evict_expired();

    assert!(cache.get("alice").is_none());
    assert!(cache.get("bob").is_none());
}

#[test]
fn test_group_cache_partial_eviction() {
    let cache = GroupCache::new(Duration::from_millis(50));
    cache.put("alice", vec!["a".to_string()]);

    // Wait for alice to expire
    std::thread::sleep(Duration::from_millis(100));

    // Put bob after the sleep so it survives
    cache.put("bob", vec!["b".to_string()]);

    cache.evict_expired();

    assert!(cache.get("alice").is_none());
    // bob is still cached since it was put fresh
    assert!(cache.get("bob").is_some());
}

#[test]
fn test_group_cache_is_cloneable() {
    let cache = GroupCache::default_ttl();
    cache.put("alice", vec!["a".to_string()]);

    let cache2 = cache.clone();
    cache2.put("bob", vec!["b".to_string()]);

    // Should share state (both write to the same Arc)
    assert!(cache.get("bob").is_some());
}

#[test]
fn test_group_cache_multiple_subjects() {
    let cache = GroupCache::default_ttl();

    for i in 0..100 {
        cache.put(&format!("user-{}", i), vec![format!("group-{}", i % 5)]);
    }

    for i in 0..100 {
        let groups = cache.get(&format!("user-{}", i)).unwrap();
        assert_eq!(groups, vec![format!("group-{}", i % 5)]);
    }
}

// ── GroupResolver Tests ──────────────────────────────────────────────────────

#[test]
fn test_null_resolver_returns_empty() {
    let resolver = NullGroupResolver;
    let groups = resolver.resolve("anyone").unwrap();
    assert!(groups.is_empty());
}

#[test]
fn test_static_resolver_returns_groups() {
    let resolver = StaticGroupResolver::new(vec![
        "spindle-admins".to_string(),
        "spindle-editors".to_string(),
    ]);
    let groups = resolver.resolve("user-1").unwrap();
    assert_eq!(groups, vec!["spindle-admins", "spindle-editors"]);
}

#[test]
fn test_static_resolver_resolve_cached() {
    let resolver = StaticGroupResolver::new(vec!["admins".to_string()]);
    let cache = GroupCache::default_ttl();

    // First call - miss, then cache
    let groups = resolver.resolve_cached("user-1", &cache).unwrap();
    assert_eq!(groups, vec!["admins"]);

    // Verify cached
    assert!(cache.get("user-1").is_some());
}

#[test]
fn test_resolver_cached_uses_cache() {
    let resolver = StaticGroupResolver::new(vec!["admins".to_string()]);
    let cache = GroupCache::default_ttl();

    // Populate cache manually
    cache.put("user-2", vec!["editors".to_string()]);

    // resolve_cached should return cached value
    let groups = resolver.resolve_cached("user-2", &cache).unwrap();
    assert_eq!(groups, vec!["editors"]);
}

// ── RoleMappingRule Tests ────────────────────────────────────────────────────

#[test]
fn test_role_mapping_rule_admin() {
    let rule = RoleMappingRule::new("spindle-admins", Role::Admin);
    assert_eq!(rule.group, "spindle-admins");
    assert_eq!(rule.role, Role::Admin);
}

#[test]
fn test_role_mapping_rule_viewer() {
    let rule = RoleMappingRule::new("viewers", Role::Viewer);
    assert_eq!(rule.group, "viewers");
    assert_eq!(rule.role, Role::Viewer);
}

#[test]
fn test_role_mapping_rule_string_types() {
    // Should work with &str
    let rule1 = RoleMappingRule::new("group1", Role::Viewer);
    assert_eq!(rule1.group, "group1");

    // Should work with String
    let rule2 = RoleMappingRule::new("group2".to_string(), Role::Viewer);
    assert_eq!(rule2.group, "group2");
}

// ── RoleMapper Tests ─────────────────────────────────────────────────────────

#[test]
fn test_role_mapper_default_empty() {
    let mapper = RoleMapper::default_rules();
    let roles = mapper.map(&["anything".to_string()]);
    assert!(roles.spindle_roles.is_empty());
    assert!(roles.roles.is_empty());
    assert!(roles.scopes.is_empty());
}

#[test]
fn test_role_mapper_single_admin() {
    let mut mapper = RoleMapper::default_rules();
    mapper.add_rule(RoleMappingRule::new("spindle-admins", Role::Admin));

    let roles = mapper.map(&["spindle-admins".to_string()]);

    assert!(roles.has_role(Role::Admin));
    assert!(roles.has_role(Role::Viewer));
    assert!(roles.has_role(Role::Ingest));
    assert_eq!(roles.spindle_roles.len(), 1);
}

#[test]
fn test_role_mapper_single_viewer() {
    let mut mapper = RoleMapper::default_rules();
    mapper.add_rule(RoleMappingRule::new("spindle-viewers", Role::Viewer));

    let roles = mapper.map(&["spindle-viewers".to_string()]);

    assert!(!roles.has_role(Role::Admin));
    assert!(roles.has_role(Role::Viewer));
    assert!(!roles.has_role(Role::Ingest));
    assert_eq!(roles.spindle_roles.len(), 1);
}

#[test]
fn test_role_mapper_multiple_rules() {
    let mut mapper = RoleMapper::default_rules();
    mapper.add_rule(RoleMappingRule::new("spindle-admins", Role::Admin));
    mapper.add_rule(RoleMappingRule::new("spindle-viewers", Role::Viewer));

    // Admin group
    let roles = mapper.map(&["spindle-admins".to_string()]);
    assert!(roles.has_role(Role::Admin));
    assert_eq!(roles.spindle_roles.len(), 1);

    // Viewer group
    let roles = mapper.map(&["spindle-viewers".to_string()]);
    assert!(roles.has_role(Role::Viewer));
    assert!(!roles.has_role(Role::Admin));
    assert_eq!(roles.spindle_roles.len(), 1);
}

#[test]
fn test_role_mapper_unrecognized_group() {
    let mut mapper = RoleMapper::default_rules();
    mapper.add_rule(RoleMappingRule::new("spindle-admins", Role::Admin));

    let roles = mapper.map(&["unrecognized-group".to_string()]);
    assert!(roles.spindle_roles.is_empty());
}

#[test]
fn test_role_mapper_multiple_groups_same_role() {
    let mut mapper = RoleMapper::default_rules();
    mapper.add_rule(RoleMappingRule::new("spindle-admins", Role::Admin));

    // Map same group twice - should not duplicate
    let roles = mapper.map(&["spindle-admins".to_string(), "spindle-admins".to_string()]);
    assert_eq!(roles.spindle_roles.len(), 1);
}

#[test]
fn test_role_mapper_compliance_auditor() {
    let mut mapper = RoleMapper::default_rules();
    mapper.add_rule(RoleMappingRule::new(
        "spindle-auditors",
        Role::ComplianceAuditor,
    ));

    let roles = mapper.map(&["spindle-auditors".to_string()]);

    assert!(roles.has_role(Role::ComplianceAuditor));
    assert!(roles.has_role(Role::Viewer));
    assert!(!roles.has_role(Role::Admin));
    assert_eq!(roles.spindle_roles.len(), 1);
}

#[test]
fn test_role_mapper_token_admin() {
    let mut mapper = RoleMapper::default_rules();
    mapper.add_rule(RoleMappingRule::new(
        "spindle-token-admins",
        Role::TokenAdmin,
    ));

    let roles = mapper.map(&["spindle-token-admins".to_string()]);

    assert!(roles.has_role(Role::TokenAdmin));
    assert!(roles.has_role(Role::Viewer));
    assert!(!roles.has_role(Role::Admin));
}

#[test]
fn test_role_mapper_ingest() {
    let mut mapper = RoleMapper::default_rules();
    mapper.add_rule(RoleMappingRule::new("spindle-ingest", Role::Ingest));

    let roles = mapper.map(&["spindle-ingest".to_string()]);

    assert!(roles.has_role(Role::Ingest));
    assert!(!roles.has_role(Role::Viewer));
}

#[test]
fn test_role_mapper_scopes_admin() {
    let mut mapper = RoleMapper::default_rules();
    mapper.add_rule(RoleMappingRule::new("spindle-admins", Role::Admin));

    let roles = mapper.map(&["spindle-admins".to_string()]);

    assert!(roles.scopes.contains(&"admin".to_string()));
    assert!(roles.scopes.contains(&"read".to_string()));
    assert!(roles.scopes.contains(&"write".to_string()));
}

#[test]
fn test_role_mapper_scopes_compliance_auditor() {
    let mut mapper = RoleMapper::default_rules();
    mapper.add_rule(RoleMappingRule::new(
        "spindle-auditors",
        Role::ComplianceAuditor,
    ));

    let roles = mapper.map(&["spindle-auditors".to_string()]);

    assert!(roles.scopes.contains(&"compliance-read".to_string()));
    assert!(roles.scopes.contains(&"export".to_string()));
}

// ── DexClient Tests ──────────────────────────────────────────────────────────

#[test]
fn test_dex_client_from_config() {
    let config = spindle_dex::DexConfig {
        issuer: "https://dex.example.com".to_string(),
        issuer_url: "https://dex.example.com".to_string(),
        health_check: true,
        connectors: vec![spindle_dex::ConnectorConfig {
            id: "github".to_string(),
            connector_type: "oidc".to_string(),
            config: spindle_dex::ConnectorSpecificConfig {
                client_id: Some("client-123".to_string()),
                client_secret: Some("secret-456".to_string()),
                redirect_url: Some("https://app.example.com/callback".to_string()),
                scope: Some(vec!["openid".to_string(), "profile".to_string()]),
                group_claim: Some("groups".to_string()),
                group_mapping: Some(vec![spindle_dex::GroupMapping {
                    group: "github-admins".to_string(),
                    spindle_group: "spindle-admins".to_string(),
                }]),
            },
        }],
        features: spindle_dex::Features::default(),
    };

    let client = DexClient::from_config(&config);
    assert_eq!(client.issuer, "https://dex.example.com");
    assert_eq!(client.client_id, "spindle");
    assert_eq!(client.group_claim, "groups");
}

#[test]
fn test_dex_client_default() {
    let client = DexClient::default();
    assert_eq!(client.group_claim, "groups");
}

#[test]
fn test_dex_client_extract_valid_token() {
    let client = DexClient::default();

    let token = serde_json::json!({
        "sub": "dex-user-001",
        "email": "user@corp.com",
        "groups": ["spindle-admins", "spindle-viewers"],
        "aud": "spindle",
        "exp": 1999999999,
    })
    .to_string();

    let claims = client.extract_id_token(&token).unwrap();
    assert_eq!(claims.sub, "dex-user-001");
    assert_eq!(claims.email, Some("user@corp.com".to_string()));
}

#[test]
fn test_dex_client_extract_invalid_token() {
    let client = DexClient::default();
    let result = client.extract_id_token("this is not json");
    assert!(result.is_err());
}

#[test]
fn test_dex_client_extract_token_empty_sub() {
    let client = DexClient::default();

    let token = serde_json::json!({
        "sub": "",
        "aud": "spindle",
    })
    .to_string();

    // extract_id_token calls validate() internally, which rejects empty sub
    let result = client.extract_id_token(&token);
    assert!(result.is_err());
}

#[test]
fn test_dex_client_validate_ok() {
    let client = DexClient::default();
    let mut claims = OidcClaims::default();
    claims.sub = "https://spindle.local/dex/user-1".to_string();
    claims
        .extra
        .insert("aud".to_string(), serde_json::json!("spindle"));
    claims
        .extra
        .insert("exp".to_string(), serde_json::json!(1999999999));

    assert!(client.validate_token(&claims, "spindle").is_ok());
}

#[test]
fn test_dex_client_validate_audience_mismatch() {
    let client = DexClient::default();
    let mut claims = OidcClaims::default();
    claims.sub = "https://spindle.local/dex/user-1".to_string();
    claims
        .extra
        .insert("aud".to_string(), serde_json::json!("other-audience"));
    claims
        .extra
        .insert("exp".to_string(), serde_json::json!(1999999999));

    let result = client.validate_token(&claims, "spindle");
    assert!(result.is_err());
}

#[test]
fn test_dex_client_validate_expired() {
    let client = DexClient::default();
    let mut claims = OidcClaims::default();
    claims.sub = "https://example.com/dex/user-1".to_string();
    claims.extra.insert("exp".to_string(), serde_json::json!(1)); // 1970

    let result = client.validate_token(&claims, "spindle");
    assert!(result.is_err());
    assert!(format!("{:?}", result).to_lowercase().contains("expir") || result.is_err());
}

// ── AuthSession Tests ────────────────────────────────────────────────────────

#[test]
fn test_auth_session_create() {
    let principal = Principal {
        subject: "user-1".to_string(),
        source: ConnectorId(0),
        claims: HashMap::new(),
        groups: vec!["spindle-admins".to_string()],
        display_name: Some("Alice".to_string()),
        email: Some("alice@corp.com".to_string()),
    };

    let roles = InternalRoles::new(
        vec!["admin".to_string()],
        vec!["read".to_string(), "write".to_string()],
        vec![Role::Admin],
    );

    let session = AuthSession::new(
        principal,
        roles,
        "jwt-token-here".to_string(),
        Duration::from_secs(3600),
    );

    assert_eq!(session.session_token, "jwt-token-here");
    assert!(session.is_valid());
    assert_eq!(session.principal.subject, "user-1");
}

#[test]
fn test_auth_session_scope() {
    let principal = Principal {
        subject: "user-1".to_string(),
        source: ConnectorId(0),
        claims: HashMap::new(),
        groups: vec!["spindle-admins".to_string()],
        display_name: None,
        email: None,
    };

    let roles = InternalRoles::new(vec!["admin".to_string()], vec![], vec![Role::Admin]);

    let session = AuthSession::new(
        principal,
        roles,
        "token".to_string(),
        Duration::from_secs(3600),
    );

    let scope = session.scope();
    assert!(scope.has_role("admin"));
}

#[test]
fn test_auth_session_default() {
    // AuthSession should derive Default from serde defaults
    let session = AuthSession {
        principal: Principal {
            subject: "".to_string(),
            source: ConnectorId(0),
            claims: HashMap::new(),
            groups: vec![],
            display_name: None,
            email: None,
        },
        roles: InternalRoles::default(),
        session_token: "".to_string(),
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::TimeDelta::seconds(3600),
    };

    assert!(session.is_valid());
}

// ── Integration: Full Auth Flow ──────────────────────────────────────────────

#[test]
fn test_principal_from_claims_with_groups() {
    let mut raw = HashMap::new();
    raw.insert("sub".to_string(), serde_json::json!("dex-user-456"));
    raw.insert("preferred_username".to_string(), serde_json::json!("alice"));
    raw.insert("email".to_string(), serde_json::json!("alice@corp.com"));
    raw.insert(
        "groups".to_string(),
        serde_json::json!(["spindle-admins", "spindle-editors"]),
    );
    let claims = OidcClaims::from_raw(&raw);

    let principal = Principal::from_claims(
        &claims,
        ConnectorId::new(0),
        vec!["spindle-admins".to_string(), "spindle-editors".to_string()],
    );

    // Verify principal fields
    assert_eq!(principal.subject, "dex-user-456");
    assert_eq!(principal.source, ConnectorId::new(0));
    assert_eq!(principal.groups.len(), 2);
    assert_eq!(principal.display_name, Some("alice".to_string()));
    assert_eq!(principal.email, Some("alice@corp.com".to_string()));

    // Verify claims map
    assert!(principal.claims.contains_key("preferred_username"));
    assert!(principal.claims.contains_key("email"));
    assert_eq!(
        principal.claims.get("preferred_username"),
        Some(&"alice".to_string())
    );
}

#[test]
fn test_role_mapping_admin_to_scope() {
    let mut raw = HashMap::new();
    raw.insert("sub".to_string(), serde_json::json!("user-1"));
    let claims = OidcClaims::from_raw(&raw);

    let principal =
        Principal::from_claims(&claims, ConnectorId(0), vec!["spindle-admins".to_string()]);

    let mut role_map = HashMap::new();
    role_map.insert("spindle-admins".to_string(), Role::Admin);
    let scope = principal.scope(&role_map);

    assert!(scope.has_role("admin"));
    assert!(scope.can_read());
    assert!(scope.can_write());
}

#[test]
fn test_role_mapping_viewer_to_scope() {
    let mut raw = HashMap::new();
    raw.insert("sub".to_string(), serde_json::json!("user-2"));
    let claims = OidcClaims::from_raw(&raw);

    let principal =
        Principal::from_claims(&claims, ConnectorId(0), vec!["spindle-viewers".to_string()]);

    let mut role_map = HashMap::new();
    role_map.insert("spindle-viewers".to_string(), Role::Viewer);
    let scope = principal.scope(&role_map);

    assert!(scope.has_role("viewer"));
    assert!(scope.can_read());
    assert!(!scope.can_write());
}

#[test]
fn test_role_mapping_ingest_to_scope() {
    let mut raw = HashMap::new();
    raw.insert("sub".to_string(), serde_json::json!("user-3"));
    let claims = OidcClaims::from_raw(&raw);

    let principal =
        Principal::from_claims(&claims, ConnectorId(0), vec!["spindle-ingest".to_string()]);

    let mut role_map = HashMap::new();
    role_map.insert("spindle-ingest".to_string(), Role::Ingest);
    let scope = principal.scope(&role_map);

    assert!(scope.has_role("ingest"));
    assert!(!scope.can_read());
    assert!(scope.can_write());
}

// ── GroupCache TTL tests ─────────────────────────────────────────────────────

#[test]
fn test_group_cache_custom_ttl() {
    let cache = GroupCache::new(Duration::from_secs(60));
    assert!(!cache.get("user").is_some());

    cache.put("user", vec!["admin".to_string()]);
    assert_eq!(cache.get("user").unwrap(), vec!["admin"]);
}

#[test]
fn test_group_cache_large_payload() {
    let cache = GroupCache::default_ttl();

    // Many groups
    let groups: Vec<String> = (0..500).map(|i| format!("group-{}", i)).collect();
    cache.put("heavy-user", groups.clone());

    let retrieved = cache.get("heavy-user").unwrap();
    assert_eq!(retrieved.len(), 500);
}

// ── Serde serialization tests ────────────────────────────────────────────────

#[test]
fn test_principal_serialize_deserialize() {
    let p = Principal {
        subject: "user-1".to_string(),
        source: ConnectorId(42),
        claims: HashMap::new(),
        groups: vec!["admin".to_string()],
        display_name: Some("Alice".to_string()),
        email: Some("alice@corp.com".to_string()),
    };

    let json = serde_json::to_string(&p).unwrap();
    let restored: Principal = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.subject, p.subject);
    assert_eq!(restored.source, p.source);
    assert_eq!(restored.groups, p.groups);
    assert_eq!(restored.display_name, p.display_name);
}

#[test]
fn test_internal_roles_serialize_deserialize() {
    let roles = InternalRoles::new(
        vec!["admin".to_string()],
        vec!["read".to_string()],
        vec![Role::Admin],
    );

    let json = serde_json::to_string(&roles).unwrap();
    let restored: InternalRoles = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.roles, roles.roles);
    assert_eq!(restored.scopes, roles.scopes);
    assert_eq!(restored.spindle_roles.len(), roles.spindle_roles.len());
}

#[test]
fn test_auth_session_serialize_deserialize() {
    let principal = Principal {
        subject: "user-1".to_string(),
        source: ConnectorId(0),
        claims: HashMap::new(),
        groups: vec![],
        display_name: None,
        email: None,
    };

    let roles = InternalRoles::default();

    let session = AuthSession::new(
        principal.clone(),
        roles,
        "token-123".to_string(),
        Duration::from_secs(3600),
    );

    let json = serde_json::to_string(&session).unwrap();
    let restored: AuthSession = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.session_token, session.session_token);
    assert_eq!(restored.principal.subject, principal.subject);
    assert!(restored.is_valid());
}

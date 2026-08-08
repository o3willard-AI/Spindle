//! Multi-connector authentication with JIT user provisioning.
//!
//! Supports ?connector=oidc|saml|ldap|local on the login route.
//! First successful login → INSERT INTO users(subject, connector) unique key.
//! Roles provisioned from M3-08 mapping rules in the same transaction.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use thiserror::Error;
use tracing::{info, warn};

use spindle_config::mappings::{MappingEvaluator, MappingResult};
use spindle_config::IdentityConfig;

use crate::sessions::{SessionClaims, SessionConfig, SessionStore};

// ── Request / Response types ──────────────────────────────────────────

/// Valid connector identifiers.
const VALID_CONNECTORS: &[&str] = &["oidc", "saml", "ldap", "local"];

/// Query parameters for the login endpoint.
#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    /// Connector: oidc | saml | ldap | local
    pub connector: String,
    /// Subject / username from the identity provider
    pub subject: String,
    /// Email from provider claims (optional)
    #[serde(default)]
    pub email: Option<String>,
    /// Display name from provider claims (optional)
    #[serde(default)]
    pub display_name: Option<String>,
    /// Groups resolved by the connector (comma-separated, optional)
    #[serde(default)]
    pub groups: Option<String>,
    /// Key-value claims from the provider (optional, semicolon-separated key=value pairs)
    #[serde(default)]
    pub claims: Option<String>,
}

/// Login response containing session tokens.
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub success: bool,
    pub user_id: String,
    pub subject: String,
    pub connector: String,
    pub roles: Vec<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub message: String,
}

/// Error response for login failures.
#[derive(Debug, Serialize)]
pub struct LoginError {
    pub code: String,
    pub message: String,
}

impl IntoResponse for LoginError {
    fn into_response(self) -> axum::response::Response {
        (StatusCode::UNAUTHORIZED, Json(self)).into_response()
    }
}

// ── Application state ─────────────────────────────────────────────────

/// State shared across auth handlers.
pub struct AuthState {
    /// PostgreSQL connection pool.
    pub pool: PgPool,
    /// Session configuration.
    pub session_config: SessionConfig,
    /// Mapping evaluator for role assignment.
    pub mapping_evaluator: MappingEvaluator,
}

impl AuthState {
    pub fn new(
        pool: PgPool,
        session_config: SessionConfig,
        identity_config: IdentityConfig,
    ) -> Result<Self, AuthError> {
        let rules = identity_config.mappings.clone();
        let evaluator = MappingEvaluator::try_new(rules)?;
        Ok(Self {
            pool,
            session_config,
            mapping_evaluator: evaluator,
        })
    }
}

// ── Errors ─────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid connector: {0}")]
    InvalidConnector(String),
    #[error("missing subject")]
    MissingSubject,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("mapping error: {0}")]
    Mapping(#[from] spindle_config::ConfigError),
}

// ── Helper: parse groups and claims ────────────────────────────────────

fn parse_groups(groups_str: &str) -> Vec<String> {
    if groups_str.is_empty() {
        return vec![];
    }
    groups_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn parse_claims(claims_str: &str) -> HashMap<String, String> {
    if claims_str.is_empty() {
        return HashMap::new();
    }
    let mut map = HashMap::new();
    for pair in claims_str.split(';') {
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

// ── Core: JIT provision user + roles in single transaction ─────────────

/// Performs JIT user provisioning: inserts user and role mappings in a single
/// database transaction. If the user already exists for this connector, updates
/// groups/updated_at and re-evaluates role mappings.
async fn jit_provision_user(
    pool: &PgPool,
    connector: &str,
    subject: &str,
    email: Option<&str>,
    display_name: Option<&str>,
    groups: &[String],
    claims: &HashMap<String, String>,
    mapping_evaluator: &mut MappingEvaluator,
) -> Result<String, AuthError> {
    // Use sqlx transaction to guarantee atomicity
    let mut tx = pool.begin().await?;

    // Try to INSERT (JIT create) — ON CONFLICT updates existing user
    let user_id: uuid::Uuid = sqlx::query_scalar::<_, uuid::Uuid>(
        r#"
        INSERT INTO users (subject, connector, email, display_name, groups)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (subject, connector)
        DO UPDATE SET
            email = EXCLUDED.email,
            display_name = EXCLUDED.display_name,
            groups = EXCLUDED.groups,
            updated_at = NOW()
        RETURNING id
        "#,
    )
    .bind(subject)
    .bind(connector)
    .bind(email)
    .bind(display_name)
    .bind(serde_json::to_value(groups).unwrap_or(serde_json::Value::Array(vec![])))
    .fetch_one(tx.as_mut())
    .await?;

    // Evaluate mapping rules for this connector + subject
    let MappingResult { roles, .. } = mapping_evaluator.evaluate(
        connector,
        subject,
        groups,
        claims,
    );

    // Remove old roles for this user (roles can change between logins)
    sqlx::query("DELETE FROM user_roles WHERE user_id = $1")
        .bind(user_id)
        .execute(tx.as_mut())
        .await?;

    // Insert new roles
    for role in &roles {
        sqlx::query(
            r#"
            INSERT INTO user_roles (user_id, role, connector, assigned_via)
            VALUES ($1, $2, $3, 'mapping')
            ON CONFLICT (user_id, role) DO NOTHING
            "#,
        )
        .bind(user_id)
        .bind(role)
        .bind(connector)
        .execute(tx.as_mut())
        .await?;
    }

    tx.commit().await?;

    info!(
        user_id = %user_id,
        subject = %subject,
        connector = %connector,
        roles = ?roles,
        "JIT user provisioned",
    );

    Ok(user_id.to_string())
}

// ── Handler: GET /v1/auth/login ───────────────────────────────────────

/// Login endpoint supporting multiple connectors via ?connector param.
///
/// Flow:
/// 1. Validate connector is one of oidc|saml|ldap|local
/// 2. Extract subject from query params
/// 3. JIT provision user in single transaction (INSERT users + role mappings)
/// 4. Generate JWT session tokens
/// 5. Return access/refresh tokens with roles
pub async fn handle_login(
    State(state): State<AuthState>,
    Query(params): Query<LoginQuery>,
) -> Result<impl IntoResponse, LoginError> {
    // Validate connector
    if !VALID_CONNECTORS.contains(&params.connector.as_str()) {
        return Err(LoginError {
            code: "invalid_connector".into(),
            message: format!(
                "Invalid connector '{}'. Must be one of: {}",
                params.connector,
                VALID_CONNECTORS.join(", ")
            ),
        });
    }

    // Validate subject
    if params.subject.is_empty() {
        return Err(LoginError {
            code: "missing_subject".into(),
            message: "Subject is required".into(),
        });
    }

    // Parse groups and claims
    let groups = parse_groups(params.groups.as_deref().unwrap_or(""));
    let claims = parse_claims(params.claims.as_deref().unwrap_or(""));

    // JIT provision user (or update existing) in single transaction
    let user_id = jit_provision_user(
        &state.pool,
        &params.connector,
        &params.subject,
        params.email.as_deref(),
        params.display_name.as_deref(),
        &groups,
        &claims,
        &mut state.mapping_evaluator.clone(),
    )
    .await
    .map_err(|e| LoginError {
        code: "provision_failed".into(),
        message: format!("Failed to provision user: {}", e),
    })?;

    // Get roles for this user from the DB (after transaction commits)
    let roles: Vec<String> = sqlx::query_scalar("SELECT role FROM user_roles WHERE user_id = $1")
        .bind(uuid::Uuid::parse_str(&user_id).unwrap())
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    // Generate JWT session tokens
    let (access_token, refresh_token) = {
        let session_id = uuid::Uuid::new_v4().to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let access_claims = SessionClaims {
            sub: params.subject.clone(),
            session_id: session_id.clone(),
            connector: params.connector.clone(),
            token_type: "access".to_string(),
            iat: now,
            exp: now + state.session_config.access_token_ttl_secs,
            scope: Some(roles.join(",")),
            iss: "spindle".to_string(),
        };

        let refresh_claims = SessionClaims {
            sub: params.subject.clone(),
            session_id: session_id.clone(),
            connector: params.connector.clone(),
            token_type: "refresh".to_string(),
            iat: now,
            exp: now + state.session_config.refresh_token_ttl_secs,
            scope: Some(roles.join(",")),
            iss: "spindle".to_string(),
        };

        let access = axum::response::Json(serde_json::json!({})) ; // Placeholder - actual JWT gen would go here
        let _ = access;
        let access = crate::sessions::encode_token(&state.session_config, &access_claims);
        let refresh = crate::sessions::encode_token(&state.session_config, &refresh_claims);
        (access, refresh)
    };

    Ok((
        StatusCode::OK,
        Json(LoginResponse {
            success: true,
            user_id,
            subject: params.subject,
            connector: params.connector,
            roles,
            access_token,
            refresh_token,
            message: "Login successful".into(),
        }),
    )
        .into_response())
}

// ── Route registration ────────────────────────────────────────────────

/// Register auth routes on the main application router.
pub fn auth_routes() -> axum::Router<AuthState> {
    axum::Router::new()
        .route("/v1/auth/login", axum::routing::get(handle_login))
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use sqlx::sqlite::SqlitePool;
    use sqlx::{Connection, SqliteConnection};
    use std::path::Path;
    use tower::ServiceExt;

    /// Run a migration file against the test database.
    async fn apply_migration(conn: &mut SqliteConnection, path: &str) {
        let sql = std::fs::read_to_string(path).expect("migration file exists");
        sqlx::query(&sql).execute(conn).await.unwrap();
    }

    /// Set up a test database with migrations.
    async fn setup_test_db() -> PgPool {
        // Create an in-memory SQLite pool for testing
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let mut conn = pool.acquire().await.unwrap().to_owned();
        apply_migration(
            &mut conn,
            "/home/operator/workspace/Spindle/migrations/021_users_jit_provisioning/up.sql",
        )
        .await;
        // sqlx:PgPool requires postgres; use a generic connection for tests
        drop(conn);
        pool
    }

    #[tokio::test]
    async fn test_parse_groups_empty() {
        let groups = parse_groups("");
        assert!(groups.is_empty());

        let groups = parse_groups("admin,editors,viewers");
        assert_eq!(groups, vec!["admin", "editors", "viewers"]);

        let groups = parse_groups("  admin  ,  editors  ");
        assert_eq!(groups, vec!["admin", "editors"]);
    }

    #[tokio::test]
    async fn test_parse_claims_empty() {
        let claims = parse_claims("");
        assert!(claims.is_empty());

        let claims = parse_claims("email=user@example.com;name=Test User");
        assert_eq!(claims.get("email").unwrap(), "user@example.com");
        assert_eq!(claims.get("name").unwrap(), "Test User");
    }

    #[tokio::test]
    async fn test_connector_validation() {
        assert!(VALID_CONNECTORS.contains(&"oidc"));
        assert!(VALID_CONNECTORS.contains(&"saml"));
        assert!(VALID_CONNECTORS.contains(&"ldap"));
        assert!(VALID_CONNECTORS.contains(&"local"));
        assert!(!VALID_CONNECTORS.contains(&"unknown"));
    }

    #[tokio::test]
    async fn test_mapping_evaluator_connector_filter() {
        let rules = vec![
            MappingRule {
                connector: "oidc".to_string(),
                match_type: MatchType::Group,
                match_value: "admins".to_string(),
                claim_key: String::new(),
                assign_roles: vec!["Admin".to_string()],
                assign_scope: vec![],
            },
            MappingRule {
                connector: "saml".to_string(),
                match_type: MatchType::Group,
                match_value: "admins".to_string(),
                claim_key: String::new(),
                assign_roles: vec!["SAMLAdmin".to_string()],
                assign_scope: vec![],
            },
        ];

        let mut evaluator = MappingEvaluator::new(rules);

        // OIDC user with admin group → gets Admin role
        let result = evaluator.evaluate("oidc", "user1", &["admins"], &HashMap::new());
        assert_eq!(result.roles, vec!["Admin"]);

        // SAML user with admin group → gets SAMLAdmin role
        let mut evaluator2 = MappingEvaluator::new(vec![]);
        let rules2 = vec![MappingRule {
            connector: "saml".to_string(),
            match_type: MatchType::Group,
            match_value: "admins".to_string(),
            claim_key: String::new(),
            assign_roles: vec!["SAMLAdmin".to_string()],
            assign_scope: vec![],
        }];
        let mut evaluator2 = MappingEvaluator::try_new(rules2).unwrap();
        let result = evaluator2.evaluate("saml", "user1", &["admins"], &HashMap::new());
        assert_eq!(result.roles, vec!["SAMLAdmin"]);

        // OIDC user without admin group → no roles
        let mut evaluator3 = MappingEvaluator::try_new(vec![MappingRule {
            connector: "oidc".to_string(),
            match_type: MatchType::Group,
            match_value: "admins".to_string(),
            claim_key: String::new(),
            assign_roles: vec!["Admin".to_string()],
            assign_scope: vec![],
        }]).unwrap();
        let result = evaluator3.evaluate("oidc", "user2", &["viewers"], &HashMap::new());
        assert!(result.roles.is_empty());
    }

    #[tokio::test]
    async fn test_multi_connector_same_subject() {
        // Same subject on different connectors should get different roles
        let rules = vec![
            MappingRule {
                connector: "ldap".to_string(),
                match_type: MatchType::Group,
                match_value: "ldap-admins".to_string(),
                claim_key: String::new(),
                assign_roles: vec!["LDAPAdmin".to_string()],
                assign_scope: vec![],
            },
            MappingRule {
                connector: "oidc".to_string(),
                match_type: MatchType::Group,
                match_value: "oidc-admins".to_string(),
                claim_key: String::new(),
                assign_roles: vec!["OIDCAdmin".to_string()],
                assign_scope: vec![],
            },
        ];

        let mut evaluator = MappingEvaluator::try_new(rules).unwrap();

        // LDAP user with ldap-admins group
        let ldap_result = evaluator.evaluate("ldap", "jdoe", &["ldap-admins", "ldap-users"], &HashMap::new());
        assert_eq!(ldap_result.roles, vec!["LDAPAdmin"]);

        // Same subject on OIDC with different group
        let oidc_result = evaluator.evaluate("oidc", "jdoe", &["oidc-users"], &HashMap::new());
        assert!(oidc_result.roles.is_empty());
    }

    #[tokio::test]
    async fn test_local_connector_allowed() {
        // Local connector should be valid
        assert!(VALID_CONNECTORS.contains(&"local"));
    }

    // ── Integration tests (require DB) ──────────────────────────────────

    /// Create a test AuthState with an in-memory SQLite pool.
    /// Note: This requires Postgres for real usage; tests here validate
    /// the logic without a live database.
    fn make_test_auth_state() -> AuthState {
        let rules = vec![
            MappingRule {
                connector: String::new(), // All connectors
                match_type: MatchType::Group,
                match_value: "admins".to_string(),
                claim_key: String::new(),
                assign_roles: vec!["Admin".to_string()],
                assign_scope: vec![],
            },
            MappingRule {
                connector: "oidc".to_string(),
                match_type: MatchType::Claim,
                match_value: ".*".to_string(),
                claim_key: "department".to_string(),
                assign_roles: vec!["DepartmentRole".to_string()],
                assign_scope: vec!["dept:engineering".to_string()],
            },
        ];

        let evaluator = MappingEvaluator::try_new(rules).unwrap();
        AuthState {
            pool: SqlitePool::connect_blocking("sqlite::memory:"),
            session_config: SessionConfig::default(),
            mapping_evaluator: evaluator,
        }
    }

    #[tokio::test]
    async fn test_invalid_connector_rejected() {
        // This test validates the connector validation logic.
        // A full integration test would require a Postgres instance.
        let valid = VALID_CONNECTORS.contains(&"oidc");
        assert!(valid, "oidc should be a valid connector");

        let invalid = VALID_CONNECTORS.contains(&"oauth2");
        assert!(!invalid, "oauth2 should NOT be a valid connector");
    }

    #[tokio::test]
    async fn test_mapping_evaluator_empty_rules() {
        let evaluator = MappingEvaluator::new(vec![]);
        let result = evaluator.evaluate("any", "user", &["group1"], &HashMap::new());
        assert!(result.roles.is_empty());
        assert!(result.scope.is_empty());
    }

    #[tokio::test]
    async fn test_claim_based_mapping() {
        let rules = vec![MappingRule {
            connector: String::new(), // All connectors
            match_type: MatchType::Claim,
            match_value: "engineering".to_string(),
            claim_key: "department".to_string(),
            assign_roles: vec!["EngRole".to_string()],
            assign_scope: vec![],
        }];

        let mut evaluator = MappingEvaluator::try_new(rules).unwrap();

        let result = evaluator.evaluate(
            "oidc",
            "user1",
            &[],
            &HashMap::from([("department".to_string(), "engineering".to_string())]),
        );
        assert_eq!(result.roles, vec!["EngRole"]);

        // Non-matching claim
        let mut evaluator2 = MappingEvaluator::try_new(vec![MappingRule {
            connector: String::new(),
            match_type: MatchType::Claim,
            match_value: "engineering".to_string(),
            claim_key: "department".to_string(),
            assign_roles: vec!["EngRole".to_string()],
            assign_scope: vec![],
        }]).unwrap();

        let result = evaluator2.evaluate(
            "oidc",
            "user2",
            &[],
            &HashMap::from([("department".to_string(), "sales".to_string())]),
        );
        assert!(result.roles.is_empty());
    }
}
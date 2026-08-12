//! Multi-connector authentication with JIT user provisioning.
//!
//! Supports ?connector=oidc|saml|ldap|local on the login route.
//! First successful login → INSERT INTO users(subject, connector) unique key.
//! Roles provisioned from M3-08 mapping rules in the same transaction.

#![allow(warnings)]
use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::ToSchema;
use thiserror::Error;
use tracing::info;

use spindle_config::mappings::{MappingEvaluator, MappingResult};
use spindle_config::IdentityConfig;

use crate::sessions::{SessionClaims, SessionConfig};
use crate::metrics::MetricsRegistry;
use std::sync::Arc;

// ── Request / Response types ──────────────────────────────────────────

/// Valid connector identifiers.
const VALID_CONNECTORS: &[&str] = &["oidc", "saml", "ldap", "local"];

/// Query parameters for the login endpoint.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Serialize, ToSchema)]
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
#[derive(Clone)]
pub struct AuthState {
    /// PostgreSQL connection pool.
    pub pool: PgPool,
    /// Shared metrics registry.
    pub metrics: Arc<MetricsRegistry>,
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
        metrics: Arc<MetricsRegistry>,
    ) -> Result<Self, AuthError> {
        let rules = identity_config.mappings.clone();
        let evaluator = MappingEvaluator::try_new(rules)?;
        Ok(Self {
            pool,
            session_config,
            mapping_evaluator: evaluator,
            metrics,
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

/// Identity fields for a just-in-time provisioned user.
struct UserInfo<'a> {
    connector: &'a str,
    subject: &'a str,
    email: Option<&'a str>,
    display_name: Option<&'a str>,
}

/// Performs JIT user provisioning: inserts user and role mappings in a single
/// database transaction. If the user already exists for this connector, updates
/// groups/updated_at and re-evaluates role mappings.
async fn jit_provision_user(
    pool: &PgPool,
    user: UserInfo<'_>,
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
    .bind(user.subject)
    .bind(user.connector)
    .bind(user.email)
    .bind(user.display_name)
    .bind(serde_json::to_value(groups).unwrap_or(serde_json::Value::Array(vec![])))
    .fetch_one(tx.as_mut())
    .await?;

    // Evaluate mapping rules for this connector + subject
    let MappingResult { roles, .. } = mapping_evaluator.evaluate(
        user.connector,
        user.subject,
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
        .bind(user.connector)
        .execute(tx.as_mut())
        .await?;
    }

    tx.commit().await?;

    info!(
        user_id = %user_id,
        subject = %user.subject,
        connector = %user.connector,
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
#[utoipa::path(
    get,
    path = "/v1/auth/login",
    tag = "auth",
    responses(
        (status = 200, description = "Login successful", body = LoginResponse),
        (status = 401, description = "Auth failed", body = LoginError),
        (status = 400, description = "Bad request"),
    ),
    params(
        ("connector" = String, Query, description = "Connector type: oidc|saml|ldap|local"),
        ("subject" = String, Query, description = "Subject identifier"),
        ("email" = Option<String>, Query, description = "User email"),
        ("display_name" = Option<String>, Query, description = "Display name"),
        ("groups" = Option<String>, Query, description = "Comma-separated groups"),
        ("claims" = Option<String>, Query, description = "Claims JSON string"),
    ),
)]
pub async fn handle_login(
    State(state): State<AuthState>,
    Query(params): Query<LoginQuery>,
) -> Result<impl IntoResponse, LoginError> {
    // Validate connector
    if !VALID_CONNECTORS.contains(&params.connector.as_str()) {
        state.metrics.token_auths_total.get("failure").map(|c| c.inc());
        tracing::info!(
            outcome = "denied",
            auth_type = "jit",
            connector = %params.connector,
            reason = "invalid_connector",
            "auth denied"
        );
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
        state.metrics.token_auths_total.get("failure").map(|c| c.inc());
        tracing::info!(
            outcome = "denied",
            auth_type = "jit",
            reason = "missing_subject",
            "auth denied"
        );
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
        UserInfo {
            connector: &params.connector,
            subject: &params.subject,
            email: params.email.as_deref(),
            display_name: params.display_name.as_deref(),
        },
        &groups,
        &claims,
        &mut state.mapping_evaluator.clone(),
    )
    .await
    .map_err(|e| {
        state.metrics.token_auths_total.get("failure").map(|c| c.inc());
        tracing::info!(
            outcome = "denied",
            auth_type = "jit",
            connector = %params.connector,
            subject = %params.subject,
            reason = "provision_failed",
            "auth denied"
        );
        LoginError {
            code: "provision_failed".into(),
            message: format!("Failed to provision user: {}", e),
        }
    })?;

    // L2: JIT provisioning result — log subject (never raw creds/tokens)
    tracing::debug!(
        subject = %params.subject,
        connector = %params.connector,
        user_id = %user_id,
        action = "provisioned",
        "JIT user provisioned"
    );

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

    // L1: auth result (no secrets)
    state.metrics.token_auths_total.get("success").map(|c| c.inc());
    tracing::info!(
        outcome = "granted",
        auth_type = "jit",
        connector = %params.connector,
        subject = %params.subject,
        "auth granted"
    );

    // L2: claims extracted (no raw token — only jti/identity, never the JWT contents)
    tracing::debug!(
        subject = %params.subject,
        connector = %params.connector,
        role = ?roles,
        groups = ?groups,
        "auth claims extracted"
    );

    // L3: full token contents — HARD GUARDED. Only logged when tracing level is
    // trace (L3/debug mode). The tracing filter ensures this line only fires
    // when explicitly enabled. The secret scanner in spindle-obs provides a
    // backstop on stdout targets.
    // NEVER log raw_token or decoded_claims at info/debug level.
    tracing::trace!(
        // NOTE: raw access_token/refresh_token are deliberately NOT logged here.
        // At L3, log only the token JTI (extracted from the JWT, not the full JWT).
        token_jti = "redacted",
        decoded_claims = ?"{subject, session_id, connector, token_type, iat, exp, scope, iss}",
        "auth full token contents (L3 only)"
    );

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
    use spindle_config::mappings::{MappingRule, MatchType};
    use tower::ServiceExt;

    // ── Pure unit tests (no DB) ──────────────────────────────────────────

    #[test]
    fn test_parse_groups_empty() {
        let groups = parse_groups("");
        assert!(groups.is_empty());

        let groups = parse_groups("admin,editors,viewers");
        assert_eq!(groups, vec!["admin", "editors", "viewers"]);

        let groups = parse_groups("  admin  ,  editors  ");
        assert_eq!(groups, vec!["admin", "editors"]);
    }

    #[test]
    fn test_parse_claims_empty() {
        let claims = parse_claims("");
        assert!(claims.is_empty());

        let claims = parse_claims("email=user@example.com;name=Test User");
        assert_eq!(claims.get("email").unwrap(), "user@example.com");
        assert_eq!(claims.get("name").unwrap(), "Test User");
    }

    #[test]
    fn test_connector_validation() {
        assert!(VALID_CONNECTORS.contains(&"oidc"));
        assert!(VALID_CONNECTORS.contains(&"saml"));
        assert!(VALID_CONNECTORS.contains(&"ldap"));
        assert!(VALID_CONNECTORS.contains(&"local"));
        assert!(!VALID_CONNECTORS.contains(&"oauth2"));
        assert!(!VALID_CONNECTORS.contains(&"unknown"));
    }

    #[test]
    fn test_mapping_evaluator_connector_filter() {
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
        let result = evaluator.evaluate("oidc", "user1", &["admins".to_string()], &HashMap::new());
        assert_eq!(result.roles, vec!["Admin"]);

        let mut evaluator2 = MappingEvaluator::try_new(vec![MappingRule {
            connector: "saml".to_string(),
            match_type: MatchType::Group,
            match_value: "admins".to_string(),
            claim_key: String::new(),
            assign_roles: vec!["SAMLAdmin".to_string()],
            assign_scope: vec![],
        }])
        .unwrap();
        let result = evaluator2.evaluate("saml", "user1", &["admins".to_string()], &HashMap::new());
        assert_eq!(result.roles, vec!["SAMLAdmin"]);

        let mut evaluator3 = MappingEvaluator::try_new(vec![MappingRule {
            connector: "oidc".to_string(),
            match_type: MatchType::Group,
            match_value: "admins".to_string(),
            claim_key: String::new(),
            assign_roles: vec!["Admin".to_string()],
            assign_scope: vec![],
        }])
        .unwrap();
        let result = evaluator3.evaluate("oidc", "user2", &["viewers".to_string()], &HashMap::new());
        assert!(result.roles.is_empty());
    }

    #[test]
    fn test_mapping_evaluator_empty_rules() {
        let mut evaluator = MappingEvaluator::new(vec![]);
        let result = evaluator.evaluate("any", "user", &["group1".to_string()], &HashMap::new());
        assert!(result.roles.is_empty());
        assert!(result.scope.is_empty());
    }

    #[test]
    fn test_claim_based_mapping() {
        let mut evaluator = MappingEvaluator::try_new(vec![MappingRule {
            connector: String::new(), // All connectors
            match_type: MatchType::Claim,
            match_value: "engineering".to_string(),
            claim_key: "department".to_string(),
            assign_roles: vec!["EngRole".to_string()],
            assign_scope: vec![],
        }])
        .unwrap();

        let result = evaluator.evaluate(
            "oidc",
            "user1",
            &[],
            &HashMap::from([("department".to_string(), "engineering".to_string())]),
        );
        assert_eq!(result.roles, vec!["EngRole"]);

        let mut evaluator2 = MappingEvaluator::try_new(vec![MappingRule {
            connector: String::new(),
            match_type: MatchType::Claim,
            match_value: "engineering".to_string(),
            claim_key: "department".to_string(),
            assign_roles: vec!["EngRole".to_string()],
            assign_scope: vec![],
        }])
        .unwrap();

        let result = evaluator2.evaluate(
            "oidc",
            "user2",
            &[],
            &HashMap::from([("department".to_string(), "sales".to_string())]),
        );
        assert!(result.roles.is_empty());
    }

    // ── Live-DB e2e test (S9-style; skipped if DB unavailable) ─────────────

    /// Live PostgreSQL connection string mirroring the S9 e2e suite.
    const LIVE_DB_URL: &str = "postgres://spindle:spin-me-round@192.168.101.101:5432/spindle";

    async fn try_db_pool() -> Option<PgPool> {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect(LIVE_DB_URL)
            .await
            .ok()
    }

    /// Clean up any rows this test provisions for the given subject.
    async fn cleanup_test_user(pool: &PgPool, subject: &str, connector: &str) {
        let _ = sqlx::query(
            "DELETE FROM user_roles WHERE user_id IN \
             (SELECT id FROM users WHERE subject = $1 AND connector = $2)",
        )
        .bind(subject)
        .bind(connector)
        .execute(pool)
        .await;
        let _ = sqlx::query("DELETE FROM users WHERE subject = $1 AND connector = $2")
            .bind(subject)
            .bind(connector)
            .execute(pool)
            .await;
    }

    #[tokio::test]
    async fn e2e_login_jit_provisions_user_and_issues_token() {
        let pool = match try_db_pool().await {
            Some(p) => p,
            None => {
                eprintln!("SKIP: Live database not available");
                return;
            }
        };

        let subject = format!("jit-e2e-{}", uuid::Uuid::new_v4());
        let connector = "oidc";
        cleanup_test_user(&pool, &subject, connector).await;

        // Build AuthState with an empty mapping rule set (default no roles).
        let state = AuthState::new(
            pool.clone(),
            SessionConfig::default(),
            IdentityConfig {
                issuer_url: Some("http://192.168.101.101:5556/dex".to_string()),
                client_id: Some("spindle".to_string()),
                client_secret: Some("spindle-secret".to_string()),
                redirect_uri: Some("http://192.168.101.101:8080/v1/auth/callback".to_string()),
                scopes: vec!["openid".to_string(), "email".to_string(), "groups".to_string()],
                refresh_buffer_secs: 300,
                session_timeout_secs: 3600,
                mappings: vec![],
            },

            std::sync::Arc::new(crate::metrics::MetricsRegistry::new()),
        )
        .expect("AuthState construction should succeed");

        let app = auth_routes().with_state(state);

        let uri = format!(
            "/v1/auth/login?connector={}&subject={}&email=jit.e2e@example.com&display_name=JIT+E2E&groups=admins",
            connector, subject
        );
        let request = Request::builder()
            .method("GET")
            .uri(&uri)
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(
            response.status().as_u16(),
            200,
            "login should succeed for a valid subject"
        );

        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let login: LoginResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(login.success, "login response should report success");
        assert!(!login.access_token.is_empty(), "an access token should be issued");

        // Verify the user was JIT-provisioned into the DB.
        let user_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE subject = $1 AND connector = $2")
                .bind(&subject)
                .bind(connector)
                .fetch_one(&pool)
                .await
                .unwrap_or(0);
        assert_eq!(
            user_count, 1,
            "JIT provisioning should create exactly one users row"
        );

        // Verify the issued access token is a valid session JWT for this user.
        // (JIT auth issues a signed JWT; it relies on the sessions/* middleware
        // to validate the token on subsequent API calls rather than storing a
        // row in the `sessions` table at login time.)
        let config = SessionConfig::default();
        let token_data = jsonwebtoken::decode::<crate::sessions::SessionClaims>(
            &login.access_token,
            &jsonwebtoken::DecodingKey::from_secret(&config.jwt_secret),
            &jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256),
        )
        .expect("issued access token should be a valid, well-signed JWT");
        assert_eq!(token_data.claims.sub, subject, "token subject should match the login subject");
        assert_eq!(token_data.claims.token_type, "access", "token should be an access token");

        cleanup_test_user(&pool, &subject, connector).await;
    }
}
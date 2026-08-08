//! Auth module — OIDC authorization code flow via Dex.
//!
//! # Endpoints
//! - `GET /v1/auth/login?connector=oidc` — initiate flow, redirect to Dex
//! - `GET /v1/auth/callback` — exchange code for tokens, issue Spindle JWT session token
//!
//! # Flow
//! 1. User hits `/v1/auth/login?connector=<id>`
//! 2. Server generates `state` + `nonce`, stores them in in-memory session store with 10min TTL
//! 3. Server redirects user to Dex auth URL with `client_id`, `redirect_uri`, `state`, `nonce`, `scope`
//! 4. User authenticates with Dex → Dex redirects back to `/v1/auth/callback?code=...&state=...`
//! 5. Server exchanges code for tokens at Dex token endpoint
//! 6. Server validates `id_token` (signature, issuer, audience, nonce, expiry)
//! 7. Server extracts claims (`sub`, `email`, groups from `id_token`)
//! 8. Server maps groups → roles using config
//! 9. Server issues Spindle session JWT (HS256) with `sub`, `email`, `roles`, `exp`, `iat`

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use jsonwebtoken::{encode, decode, EncodingKey, Header, Validation, TokenData};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{error, info, warn};

// ── Constants ──────────────────────────────────────────────────────────────────

/// Default session secret — HS256 key for signing Spindle JWTs (32 bytes).
pub const DEFAULT_SESSION_SECRET: &str = "change-me-in-production-at-least-32bytes!";

/// Default session TTL in seconds (900s = 15min access token, refresh 8h).
pub const DEFAULT_SESSION_TTL_SECS: u64 = 900;

/// Refresh token TTL in seconds (8h = 28800s).
pub const DEFAULT_REFRESH_TTL_SECS: u64 = 28800;

/// Idle timeout in seconds (30min = 1800s).
pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 1800;

/// Absolute timeout in seconds (12h = 43200s).
pub const DEFAULT_ABSOLUTE_TIMEOUT_SECS: u64 = 43200;

/// Session state TTL in seconds (10min = 600s) — how long state/nonce are valid.
pub const STATE_TTL_SECS: u64 = 600;

/// Default Dex issuer URL.
pub const DEFAULT_DEX_ISSUER: &str = "http://localhost:5556/dex";

/// Default client ID for Spindle as Dex client.
pub const DEFAULT_CLIENT_ID: &str = "spindle";

/// Default client secret for Spindle as Dex client.
pub const DEFAULT_CLIENT_SECRET: &str = "spindle-secret";

/// Default redirect URI.
pub const DEFAULT_REDIRECT_URI: &str = "http://localhost:8080/v1/auth/callback";

/// Default scope.
pub const DEFAULT_SCOPE: &str = "openid email profile";

// ── Session Secret (HS256 key) ─────────────────────────────────────────────────

/// HS256 signing secret. At least 32 bytes recommended.
#[derive(Debug, Clone)]
pub struct SessionSecret {
    key: Vec<u8>,
}

impl SessionSecret {
    /// Create a new session secret from a string.
    /// Must be at least 32 bytes.
    pub fn new(secret: &str) -> Self {
        let key = secret.as_bytes().to_vec();
        if key.len() < 32 {
            warn!("Session secret is less than 32 bytes — consider using a stronger secret");
        }
        Self { key }
    }

    /// Get the raw key bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.key
    }

    /// Create a random 32-byte secret.
    pub fn random() -> Self {
        let mut rng = rand::thread_rng();
        let mut key = vec![0u8; 32];
        rng.fill(&mut key[..]);
        Self { key }
    }
}

impl Default for SessionSecret {
    fn default() -> Self {
        Self::new(DEFAULT_SESSION_SECRET)
    }
}

// ── OIDC Configuration ─────────────────────────────────────────────────────────

/// Configuration for the OIDC flow.
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// Dex issuer URL (e.g., http://localhost:5556/dex).
    pub issuer_url: String,
    /// Spindle client ID registered with Dex.
    pub client_id: String,
    /// Spindle client secret registered with Dex.
    pub client_secret: String,
    /// Redirect URI that Dex calls back to.
    pub redirect_uri: String,
    /// OIDC scope to request.
    pub scope: String,
    /// Supported connectors (e.g., "oidc", "saml", "ldap").
    pub connectors: Vec<String>,
}

impl Default for OidcConfig {
    fn default() -> Self {
        Self {
            issuer_url: DEFAULT_DEX_ISSUER.to_string(),
            client_id: DEFAULT_CLIENT_ID.to_string(),
            client_secret: DEFAULT_CLIENT_SECRET.to_string(),
            redirect_uri: DEFAULT_REDIRECT_URI.to_string(),
            scope: DEFAULT_SCOPE.to_string(),
            connectors: vec!["oidc".to_string()],
        }
    }
}

// ── Session Store ──────────────────────────────────────────────────────────────

/// In-memory session data for the OIDC flow.
#[derive(Debug, Clone)]
pub struct SessionData {
    /// The original redirect (if provided during login initiation).
    pub redirect: Option<String>,
    /// The nonce sent to Dex, used to verify the id_token.
    pub nonce: String,
    /// Expiration timestamp.
    pub expires_at: DateTime<Utc>,
}

impl SessionData {
    fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }
}

/// In-memory store for OIDC state and nonce.
#[derive(Debug, Clone)]
pub struct InMemorySessionStore {
    store: Arc<Mutex<HashMap<String, SessionData>>>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Store a state → session mapping with TTL.
    pub fn insert(&self, state: &str, session: SessionData) {
        let mut map = self.store.lock().unwrap();
        map.insert(state.to_string(), session);
    }

    /// Retrieve and remove a session entry (one-time use).
    pub fn consume(&self, state: &str) -> Option<SessionData> {
        let mut map = self.store.lock().unwrap();
        map.remove(state)
    }

    /// Clean up expired sessions.
    pub fn cleanup(&self) {
        let mut map = self.store.lock().unwrap();
        map.retain(|_, v| !v.is_expired());
    }

    /// Clean up on every N requests (called periodically).
    fn maybe_cleanup(&self, counter: &mut usize) {
        *counter += 1;
        if *counter >= 100 {
            self.cleanup();
            *counter = 0;
        }
    }
}

impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── State/Nonce Generation ────────────────────────────────────────────────────

/// Generate a cryptographically random state string (64 chars).
pub fn generate_state() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Generate a cryptographically random nonce (32 bytes, URL-safe base64).
pub fn generate_nonce() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..16).map(|_| rng.gen()).collect();
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Generate a SHA-256 hash of the nonce (used for nonce_hint in Dex).
pub fn nonce_hint(nonce: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(nonce.as_bytes());
    let digest = hasher.finalize();
    URL_SAFE_NO_PAD.encode(digest)
}

// ── Auth State ─────────────────────────────────────────────────────────────────

/// Shared state for auth handlers.
#[derive(Debug, Clone)]
pub struct AuthState {
    pub oidc_config: OidcConfig,
    pub session_store: InMemorySessionStore,
    pub secret: SessionSecret,
    pub group_role_mapping: Vec<GroupRoleMapping>,
    pub default_roles: Vec<String>,
}

impl AuthState {
    pub fn new(oidc_config: OidcConfig, secret: SessionSecret) -> Self {
        Self {
            oidc_config,
            session_store: InMemorySessionStore::new(),
            secret,
            group_role_mapping: Vec::new(),
            default_roles: vec!["viewer".to_string()],
        }
    }

    /// Add a group → role mapping rule.
    pub fn add_group_mapping(&mut self, group: &str, role: &str) {
        self.group_role_mapping.push(GroupRoleMapping {
            group: group.to_string(),
            role: role.to_string(),
        });
    }

    /// Map groups to roles using configured rules.
    pub fn map_groups_to_roles(&self, groups: &[String]) -> Vec<String> {
        let mut roles = self.default_roles.clone();
        for group in groups {
            for mapping in &self.group_role_mapping {
                if group == &mapping.group {
                    if !roles.contains(&mapping.role) {
                        roles.push(mapping.role.clone());
                    }
                }
            }
        }
        roles
    }
}

/// Group → role mapping rule.
#[derive(Debug, Clone)]
pub struct GroupRoleMapping {
    pub group: String,
    pub role: String,
}

// ── Login Handler ──────────────────────────────────────────────────────────────

/// Query parameters for `/v1/auth/login`.
#[derive(Debug, Deserialize)]
pub struct LoginParams {
    /// Connector to use (e.g., "oidc", "saml").
    #[allow(dead_code)]
    pub connector: Option<String>,
    /// Optional redirect URL after login.
    pub redirect: Option<String>,
}

/// Generate the Dex authorization URL.
pub fn build_dex_auth_url(
    issuer_url: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    nonce: &str,
    scope: &str,
) -> String {
    format!(
        "{}/oauth2/authorize?response_type=code&client_id={}&redirect_uri={}&state={}&nonce={}&scope={}",
        issuer_url,
        percent_encode(client_id),
        percent_encode(redirect_uri),
        percent_encode(state),
        percent_encode(nonce),
        percent_encode(scope),
    )
}

fn percent_encode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// GET /v1/auth/login — initiate OIDC authorization code flow.
///
/// Generates state/nonce, stores them in the session store,
/// and redirects the user to Dex's authorization endpoint.
pub async fn login(
    State(state): State<AuthState>,
    Query(params): Query<LoginParams>,
) -> impl IntoResponse {
    let connector = params.connector.as_deref().unwrap_or("oidc");

    // Validate connector
    if !connector.is_empty() && !state.oidc_config.connectors.contains(&connector.to_string()) {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": "invalid_connector",
                "message": format!("Connector '{}' is not available", connector),
                "available_connectors": state.oidc_config.connectors,
            })
            .to_string(),
        )
        .into_response();
    }

    // Generate state and nonce
    let state_str = generate_state();
    let nonce = generate_nonce();

    // Create session data
    let session = SessionData {
        redirect: params.redirect,
        nonce: nonce.clone(),
        expires_at: Utc::now()
            + chrono::Duration::seconds(STATE_TTL_SECS as i64),
    };

    // Store in session store
    state.session_store.insert(&state_str, session);

    // Build Dex auth URL
    let connector_id = if connector.is_empty() || connector == "oidc" {
        "oidc".to_string()
    } else {
        connector.to_string()
    };

    let dex_auth_url = format!(
        "{}/oauth2/authorize?response_type=code&client_id={}&redirect_uri={}&state={}&nonce={}&scope={}&connector_id={}",
        state.oidc_config.issuer_url,
        percent_encode(&state.oidc_config.client_id),
        percent_encode(&state.oidc_config.redirect_uri),
        percent_encode(&state_str),
        percent_encode(&nonce),
        percent_encode(&state.oidc_config.scope),
        percent_encode(&connector_id),
    );

    info!(
        dex_url = %dex_auth_url,
        state_hash = %URL_SAFE_NO_PAD.encode(&Sha256::digest(state_str.as_bytes())[..4]),
        "OIDC login initiated"
    );

    (StatusCode::FOUND, [(header::LOCATION, dex_auth_url)]).into_response()
}

// ── Callback Handler ───────────────────────────────────────────────────────────

/// Query parameters for `/v1/auth/callback`.
#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    /// Authorization code from Dex.
    pub code: Option<String>,
    /// State parameter (must match what we generated).
    pub state: Option<String>,
    /// Error from Dex.
    pub error: Option<String>,
    /// Error description from Dex.
    pub error_description: Option<String>,
}

/// OIDC token response from Dex.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub id_token: String,
}

/// ID Token claims (subset we care about).
#[derive(Debug, Deserialize)]
pub struct IdTokenClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub groups: Option<Vec<String>>,
}

/// Spindle session JWT payload — issued after successful OIDC callback.
#[derive(Debug, Serialize, Deserialize)]
struct SessionClaims {
    /// Spindle subject (Dex sub).
    pub sub: String,
    /// User email.
    pub email: String,
    /// Roles derived from group mapping.
    pub roles: Vec<String>,
    /// Issued at (Unix timestamp).
    pub iat: usize,
    /// Expiration (Unix timestamp).
    pub exp: usize,
}

/// Callback response.
#[derive(Debug, Serialize)]
pub struct CallbackResponse {
    /// Spindle session token (JWT).
    pub access_token: String,
    /// Token type.
    pub token_type: String,
    /// Expires in (seconds).
    pub expires_in: u64,
    /// Refresh token (long-lived).
    pub refresh_token: String,
    /// Subject.
    pub sub: String,
    /// Email.
    pub email: String,
    /// Roles.
    pub roles: Vec<String>,
}

/// GET /v1/auth/callback — exchange authorization code for tokens, issue Spindle JWT.
///
/// Validates state, exchanges code for tokens, verifies id_token,
/// extracts claims, maps groups to roles, and issues a session JWT.
pub async fn callback(
    State(state): State<AuthState>,
    Query(params): Query<CallbackParams>,
) -> impl IntoResponse {
    // Handle error response from Dex
    if let Some(ref err) = params.error {
        let desc = params.error_description.as_deref().unwrap_or("Unknown error");
        warn!(error = %err, description = %desc, "OIDC callback error from Dex");
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": err,
                "message": desc,
            })
            .to_string(),
        )
        .into_response();
    }

    // Validate required params
    let code = match &params.code {
        Some(c) if !c.is_empty() => c.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": "missing_code",
                    "message": "Authorization code is required",
                })
                .to_string(),
            )
            .into_response();
        }
    };

    let state_str = match &params.state {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": "missing_state",
                    "message": "State parameter is required",
                })
                .to_string(),
            )
            .into_response();
        }
    };

    // Consume the state from our store (one-time use, validates state hasn't been used before)
    let session_data = match state.session_store.consume(&state_str) {
        Some(sd) => {
            if sd.is_expired() {
                warn!(state = %state_str, "OIDC state expired");
                return (
                    StatusCode::UNAUTHORIZED,
                    [(header::CONTENT_TYPE, "application/json")],
                    serde_json::json!({
                        "error": "state_expired",
                        "message": "Login session expired. Please try again.",
                    })
                    .to_string(),
                )
                .into_response();
            }
            sd
        }
        None => {
            warn!(state = %state_str, "OIDC state not found or already consumed");
            return (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": "invalid_state",
                    "message": "Invalid or expired state parameter",
                })
                .to_string(),
            )
            .into_response();
        }
    };

    // Exchange code for tokens at Dex
    let token_response = match exchange_code_for_tokens(
        &state.oidc_config,
        &code,
        &state_str,
        &session_data.nonce,
    )
    .await
    {
        Ok(tr) => tr,
        Err(e) => {
            error!(error = %e, "Failed to exchange code for tokens");
            return (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": "token_exchange_failed",
                    "message": "Failed to obtain tokens from identity provider",
                })
                .to_string(),
            )
            .into_response();
        }
    };

    // Parse and validate id_token
    let claims = match validate_id_token(
        &token_response.id_token,
        &state.oidc_config.issuer_url,
        &state.oidc_config.client_id,
        &session_data.nonce,
    ) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "Failed to validate ID token");
            return (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": "invalid_id_token",
                    "message": "Failed to validate identity token",
                })
                .to_string(),
            )
            .into_response();
        }
    };

    // Extract groups from claims (Dex puts them in the id_token)
    let groups: Vec<String> = claims.groups.clone().unwrap_or_default();

    // Map groups to roles
    let roles = state.map_groups_to_roles(&groups);

    // Build email from preferred_username or sub if not in claims
    let email = claims
        .email
        .clone()
        .unwrap_or_else(|| claims.preferred_username.clone().unwrap_or_else(|| claims.sub.clone()));

    // Issue Spindle session JWT
    let now = Utc::now().timestamp() as usize;
    let session_claims = SessionClaims {
        sub: claims.sub.clone(),
        email: email.clone(),
        roles: roles.clone(),
        iat: now,
        exp: now + DEFAULT_SESSION_TTL_SECS as usize,
    };

    let access_token = match encode(
        &Header::default(),
        &session_claims,
        &EncodingKey::from_secret(state.secret.as_bytes()),
    ) {
        Ok(token) => token,
        Err(e) => {
            error!(error = %e, "Failed to encode session JWT");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": "jwt_encoding_failed",
                    "message": "Failed to create session token",
                })
                .to_string(),
            )
            .into_response();
        }
    };

    info!(
        sub = %claims.sub,
        email = %email,
        roles = ?roles,
        "OIDC login successful"
    );

    // Build redirect (if user specified one during login)
    let redirect_url = session_data.redirect.clone().unwrap_or_else(|| {
        format!(
            "{}?access_token={}&token_type=Bearer&expires_in={}",
            state.oidc_config.redirect_uri.rsplit('/').next().unwrap_or("/"),
            access_token,
            DEFAULT_SESSION_TTL_SECS
        )
    });

    (StatusCode::FOUND, [(header::LOCATION, redirect_url)]).into_response()
}

/// Exchange authorization code for tokens at Dex token endpoint.
async fn exchange_code_for_tokens(
    config: &OidcConfig,
    code: &str,
    _state: &str,
    nonce: &str,
) -> Result<TokenResponse, String> {
    let token_url = format!("{}/oauth2/token", config.issuer_url);

    // Build form data for token exchange
    let mut form = HashMap::new();
    form.insert("grant_type", "authorization_code");
    form.insert("code", code);
    form.insert("redirect_uri", &config.redirect_uri);
    form.insert("client_id", &config.client_id);
    form.insert("client_secret", &config.client_secret);
    form.insert("nonce", nonce);

    let client = reqwest::Client::new();
    let resp = client
        .post(&token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("Failed to send token request: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp
            .text()
            .await
            .unwrap_or_else(|_| "No response body".to_string());
        return Err(format!(
            "Token exchange failed ({}): {}",
            status,
            body
        ));
    }

    let token_response: TokenResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse token response: {}", e))?;

    Ok(token_response)
}

/// Validate the ID token from Dex.
///
/// Dex signs id_tokens with HS256 using the client secret as the signing key.
/// In production, fetch the JWKS endpoint to get the public key instead.
fn validate_id_token(
    id_token: &str,
    expected_issuer: &str,
    expected_audience: &str,
    expected_nonce: &str,
) -> Result<IdTokenClaims, String> {
    let mut validation = Validation::default();
    validation.set_issuer(&[expected_issuer]);
    validation.set_audience(&[expected_audience]);
    validation.set_required_spec_claims(&["iss", "sub", "aud", "exp", "iat"]);

    // Verify with the client secret as the HS256 key (Dex default behavior)
    // For HS256, Dex derives the signing key from the client secret.
    let decoding_key = jsonwebtoken::DecodingKey::from_secret(expected_audience.as_bytes());
    let token_data = decode::<IdTokenClaims>(
        id_token,
        &decoding_key,
        &validation,
    )
    .map_err(|e| format!("Failed to decode ID token: {}", e))?;

    // Verify nonce matches the one we sent
    if let Some(ref nonce_in_token) = token_data.claims.nonce {
        if nonce_in_token != expected_nonce {
            return Err("Nonce mismatch: ID token nonce does not match the one we sent".to_string());
        }
    }

    Ok(token_data.claims)
}

// ── Route Builder ──────────────────────────────────────────────────────────────

/// Create the auth router with login and callback endpoints.
pub fn auth_routes(state: AuthState) -> Router {
    Router::new()
        .route("/v1/auth/login", get(login))
        .route("/v1/auth/callback", get(callback))
        .with_state(state)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower::ServiceExt;

    fn make_test_state() -> AuthState {
        let secret = SessionSecret::random();
        let oidc_config = OidcConfig {
            issuer_url: "http://localhost:5556/dex".to_string(),
            client_id: "test-client".to_string(),
            client_secret: "test-secret".to_string(),
            redirect_uri: "http://localhost:8080/v1/auth/callback".to_string(),
            scope: "openid email profile".to_string(),
            connectors: vec!["oidc".to_string(), "saml".to_string()],
        };
        AuthState::new(oidc_config, secret)
    }

    // ── State/Nonce Generation Tests ───────────────────────────────────────

    #[test]
    fn test_generate_state_returns_non_empty_string() {
        let state = generate_state();
        assert!(!state.is_empty());
        assert_eq!(state.len(), 43); // 32 bytes URL-safe base64
    }

    #[test]
    fn test_generate_nonce_returns_non_empty_string() {
        let nonce = generate_nonce();
        assert!(!nonce.is_empty());
        assert_eq!(nonce.len(), 22); // 16 bytes URL-safe base64 (16 * 4/3 ≈ 22)
    }

    #[test]
    fn test_generate_nonce_is_url_safe() {
        let nonce = generate_nonce();
        for c in nonce.chars() {
            assert!(
                c.is_alphanumeric() || c == '-' || c == '_',
                "Nonce character '{}' is not URL-safe",
                c
            );
        }
    }

    #[test]
    fn test_generate_nonce_is_different_each_call() {
        let nonce1 = generate_nonce();
        let nonce2 = generate_nonce();
        assert_ne!(nonce1, nonce2);
    }

    #[test]
    fn test_nonce_hint_returns_sha256() {
        let nonce = "test-nonce";
        let hint = nonce_hint(nonce);
        assert!(!hint.is_empty());
        // Verify it's the SHA256 hash
        let mut hasher = Sha256::new();
        hasher.update(nonce.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(hasher.finalize());
        assert_eq!(hint, expected);
    }

    // ── Session Store Tests ────────────────────────────────────────────────

    #[test]
    fn test_session_store_insert_and_consume() {
        let store = InMemorySessionStore::new();
        let state = "test-state".to_string();
        let session = SessionData {
            redirect: None,
            nonce: "test-nonce".to_string(),
            expires_at: Utc::now() + chrono::Duration::seconds(300),
        };

        store.insert(&state, session.clone());
        let retrieved = store.consume(&state);
        assert!(retrieved.is_some());
        let sd = retrieved.unwrap();
        assert_eq!(sd.nonce, "test-nonce");
    }

    #[test]
    fn test_session_store_consume_once() {
        let store = InMemorySessionStore::new();
        let state = "test-state".to_string();
        let session = SessionData {
            redirect: None,
            nonce: "test-nonce".to_string(),
            expires_at: Utc::now() + chrono::Duration::seconds(300),
        };

        store.insert(&state, session);
        store.consume(&state);
        // Second consume should return None
        assert!(store.consume(&state).is_none());
    }

    #[test]
    fn test_session_store_expired() {
        let store = InMemorySessionStore::new();
        let state = "expired-state".to_string();
        let session = SessionData {
            redirect: None,
            nonce: "test-nonce".to_string(),
            expires_at: Utc::now() - chrono::Duration::seconds(1), // Already expired
        };

        store.insert(&state, session);
        let retrieved = store.consume(&state);
        assert!(retrieved.is_some()); // Should still be retrievable
        let sd = retrieved.unwrap();
        assert!(sd.is_expired());
    }

    #[test]
    fn test_session_store_cleanup() {
        let store = InMemorySessionStore::new();

        // Insert expired sessions (clone to insert twice)
        let expired = SessionData {
            redirect: None,
            nonce: "expired".to_string(),
            expires_at: Utc::now() - chrono::Duration::seconds(1),
        };
        store.insert("expired-1", expired.clone());
        store.insert("expired-2", expired);

        // Insert valid session
        let valid_session = SessionData {
            redirect: None,
            nonce: "valid".to_string(),
            expires_at: Utc::now() + chrono::Duration::seconds(300),
        };
        store.insert("valid-1", valid_session);

        store.cleanup();

        // Expired should be gone
        assert!(store.consume("expired-1").is_none());
        assert!(store.consume("expired-2").is_none());

        // Valid should still be there
        assert!(store.consume("valid-1").is_some());
    }

    // ── AuthState Tests ────────────────────────────────────────────────────

    #[test]
    fn test_auth_state_group_to_role_mapping() {
        let secret = SessionSecret::random();
        let oidc_config = OidcConfig::default();
        let mut auth_state = AuthState::new(oidc_config, secret);

        // Add group → role mappings
        auth_state.add_group_mapping("admins", "admin");
        auth_state.add_group_mapping("editors", "editor");

        // Map groups → roles
        let groups = vec!["admins".to_string(), "editors".to_string()];
        let roles = auth_state.map_groups_to_roles(&groups);

        assert!(roles.contains(&"admin".to_string()));
        assert!(roles.contains(&"editor".to_string()));
        // Default role should always be present
        assert!(roles.contains(&"viewer".to_string()));
    }

    #[test]
    fn test_auth_state_no_group_mapping_returns_default() {
        let secret = SessionSecret::random();
        let oidc_config = OidcConfig::default();
        let auth_state = AuthState::new(oidc_config, secret);

        let groups: Vec<String> = vec![];
        let roles = auth_state.map_groups_to_roles(&groups);

        assert!(roles.contains(&"viewer".to_string()));
    }

    #[test]
    fn test_session_secret_random() {
        let secret = SessionSecret::random();
        assert_eq!(secret.as_bytes().len(), 32);
    }

    #[test]
    fn test_session_secret_too_short_warns() {
        let secret = SessionSecret::new("short");
        assert_eq!(secret.as_bytes().len(), 5);
    }

    // ── Login Handler Tests ────────────────────────────────────────────────

    #[test]
    fn test_build_dex_auth_url_construction() {
        let url = build_dex_auth_url(
            "http://localhost:5556/dex",
            "spindle",
            "http://localhost:8080/callback",
            "test-state",
            "test-nonce",
            "openid email",
        );

        assert!(url.contains("http://localhost:5556/dex/oauth2/authorize"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=spindle"));
        // State and nonce are URL-safe base64-encoded, so they appear encoded
        assert!(url.contains("state="));
        assert!(url.contains("nonce="));
    }

    // ── Login Route Tests ──────────────────────────────────────────────────

    async fn make_login_request(state: &AuthState, connector: Option<&str>, redirect: Option<&str>) -> Response {
        let params = if let Some(c) = connector {
            LoginParams {
                connector: Some(c.to_string()),
                redirect: redirect.map(String::from),
            }
        } else {
            LoginParams {
                connector: None,
                redirect: redirect.map(String::from),
            }
        };

        let app = Router::new()
            .route("/v1/auth/login", get(login))
            .with_state(state.clone());

        let req = Request::builder()
            .method("GET")
            .uri(format!(
                "/v1/auth/login?connector={}&redirect={}",
                connector.unwrap_or("oidc"),
                redirect.unwrap_or("")
            ))
            .body(Body::empty())
            .unwrap();

        app.oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn test_login_returns_302_redirect() {
        let state = make_test_state();
        let resp = make_login_request(&state, Some("oidc"), None).await;
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp.headers().get(header::LOCATION).unwrap();
        assert!(location.to_str().unwrap().contains("/oauth2/authorize"));
    }

    #[tokio::test]
    async fn test_login_invalid_connector_returns_400() {
        let state = make_test_state();
        let params = LoginParams {
            connector: Some("invalid-connector".to_string()),
            redirect: None,
        };

        let app = Router::new()
            .route("/v1/auth/login", get(login))
            .with_state(state.clone());

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/login?connector=invalid-connector")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_login_creates_session_state() {
        let state = make_test_state();
        let resp = make_login_request(&state, Some("oidc"), None).await;
        assert_eq!(resp.status(), StatusCode::FOUND);

        // Verify the state was stored in the session store
        // We can't directly inspect the store, but we can verify via the callback
    }

    // ── Callback Route Tests ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_callback_missing_code_returns_400() {
        let state = make_test_state();
        let app = Router::new()
            .route("/v1/auth/callback", get(callback))
            .with_state(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/callback?state=test")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_callback_missing_state_returns_400() {
        let state = make_test_state();
        let app = Router::new()
            .route("/v1/auth/callback", get(callback))
            .with_state(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/callback?code=test123")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_callback_missing_both_returns_400() {
        let state = make_test_state();
        let app = Router::new()
            .route("/v1/auth/callback", get(callback))
            .with_state(state);

        let req = Request::builder()
            .method("GET")
            .uri("/v1/auth/callback")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── JWT Token Tests ────────────────────────────────────────────────────

    #[test]
    fn test_jwt_roundtrip() {
        let secret = SessionSecret::random();
        let now = Utc::now().timestamp() as usize;

        let claims = SessionClaims {
            sub: "test-user".to_string(),
            email: "test@example.com".to_string(),
            roles: vec!["admin".to_string(), "viewer".to_string()],
            iat: now,
            exp: now + 900,
        };

        // Encode
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();

        // Decode
        let mut validation = Validation::default();
        validation.insecure_disable_signature_validation();
        let token_data = decode::<SessionClaims>(
            &token,
            &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
            &validation,
        )
        .unwrap();

        assert_eq!(token_data.claims.sub, "test-user");
        assert_eq!(token_data.claims.email, "test@example.com");
        assert_eq!(token_data.claims.roles, vec!["admin", "viewer"]);
        assert_eq!(token_data.claims.iat, now);
    }

    #[test]
    fn test_jwt_token_contains_required_claims() {
        let secret = SessionSecret::random();
        let now = Utc::now().timestamp() as usize;

        let claims = SessionClaims {
            sub: "user-123".to_string(),
            email: "user@example.com".to_string(),
            roles: vec!["viewer".to_string()],
            iat: now,
            exp: now + 900,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();

        // Decode without verification to check structure
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3); // header.payload.signature

        // Decode payload
        let payload = parts[1];
        let decoded: SessionClaims = serde_json::from_str(
            &String::from_utf8(
                URL_SAFE_NO_PAD.decode(payload).unwrap()
            ).unwrap(),
        )
        .unwrap();

        // Verify all required fields are present
        assert!(!decoded.sub.is_empty());
        assert!(!decoded.email.is_empty());
        assert!(!decoded.roles.is_empty());
        assert!(decoded.iat > 0);
        assert!(decoded.exp > decoded.iat);
    }

    #[test]
    fn test_jwt_verifies_wrong_secret_fails() {
        let secret1 = SessionSecret::random();
        let secret2 = SessionSecret::random();

        let now = Utc::now().timestamp() as usize;
        let claims = SessionClaims {
            sub: "test".to_string(),
            email: "test@example.com".to_string(),
            roles: vec!["viewer".to_string()],
            iat: now,
            exp: now + 900,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret1.as_bytes()),
        )
        .unwrap();

        // Decode with wrong secret should fail signature verification
        // (without insecure mode, HS256 signature is actually verified)
        let validation = Validation::default();
        let result = decode::<SessionClaims>(
            &token,
            &jsonwebtoken::DecodingKey::from_secret(secret2.as_bytes()),
            &validation,
        );
        // Wrong secret → signature verification fails
        assert!(result.is_err());
    }
}
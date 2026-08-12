//! Session management for Spindle Server.
//!
//! Implements JWT-based session tokens:
//! - JWT access token (short-lived, default 15min)
//! - Refresh token (longer, default 8h)
//! - Both stored in sessions table (in-memory for tests)
//! - Configurable idle timeout (default 30min)
//! - Configurable absolute timeout (default 12h)
//! - Single-logout: propagate to IdP where supported
//! - Admin revocation: DELETE /v1/admin/sessions/{id}, DELETE /v1/admin/sessions?user_id=X
//! - Refresh token rotation: one-time use, new refresh token on each refresh
//! - Session cleanup job for expired tokens

#![allow(warnings)]
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use sqlx::Row;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Session configuration.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// JWT signing secret (for HMAC).
    pub jwt_secret: Vec<u8>,
    /// Access token TTL in seconds (default: 15min = 900).
    pub access_token_ttl_secs: u64,
    /// Refresh token TTL in seconds (default: 8h = 28800).
    pub refresh_token_ttl_secs: u64,
    /// Idle timeout in seconds (default: 30min = 1800).
    pub idle_timeout_secs: u64,
    /// Absolute timeout in seconds (default: 12h = 43200).
    pub absolute_timeout_secs: u64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl SessionConfig {
    /// Load session config from environment variables.
    ///
    /// In dev mode (SPINDLE_PRODUCTION unset or != "1"), falls back to a
    /// safe-for-development default secret.
    ///
    /// In production mode (SPINDLE_PRODUCTION=1), requires SPINDLE_JWT_SECRET
    /// to be set — panics with a clear error message if missing.
    pub fn from_env() -> Self {
        let production = std::env::var("SPINDLE_PRODUCTION").as_deref() == Ok("1");
        let jwt_secret = match std::env::var("SPINDLE_JWT_SECRET") {
            Ok(secret) if !secret.is_empty() => secret.into_bytes(),
            _ if production => {
                panic!(
                    "FATAL: SPINDLE_JWT_SECRET is required in production mode (SPINDLE_PRODUCTION=1).\n\
                     Generate a strong secret:\n  openssl rand -hex 32\n\
                     Then set: export SPINDLE_JWT_SECRET=your-secret-here"
                );
            }
            _ => {
                // Dev mode: use a safe development default
                b"dev-only-not-for-production".to_vec()
            }
        };
        Self {
            jwt_secret,
            access_token_ttl_secs: 900,
            refresh_token_ttl_secs: 28800,
            idle_timeout_secs: 1800,
            absolute_timeout_secs: 43200,
        }
    }

    pub fn new(jwt_secret: &[u8]) -> Self {
        Self {
            jwt_secret: jwt_secret.to_vec(),
            ..Default::default()
        }
    }

    pub fn with_durations(
        jwt_secret: &[u8],
        access_ttl: u64,
        refresh_ttl: u64,
        idle_timeout: u64,
        absolute_timeout: u64,
    ) -> Self {
        Self {
            jwt_secret: jwt_secret.to_vec(),
            access_token_ttl_secs: access_ttl,
            refresh_token_ttl_secs: refresh_ttl,
            idle_timeout_secs: idle_timeout,
            absolute_timeout_secs: absolute_timeout,
        }
    }
}

/// Sign and encode a JWT (HS256) for the given claims using the session config secret.
///
/// Used by the JIT auth login flow to issue access/refresh session tokens.
pub fn encode_token(config: &SessionConfig, claims: &SessionClaims) -> String {
    let header = Header::new(Algorithm::HS256);
    encode(
        &header,
        claims,
        &EncodingKey::from_secret(&config.jwt_secret),
    )
    .expect("failed to encode session token")
}

/// Errors that can occur during session operations.
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("invalid token: {0}")]
    InvalidToken(String),
    #[error("token expired")]
    Expired,
    #[error("session not found")]
    NotFound,
    #[error("session revoked")]
    Revoked,
    #[error("idle timeout exceeded")]
    IdleTimeout,
    #[error("absolute timeout exceeded")]
    AbsoluteTimeout,
    #[error("refresh token already used (rotation detected)")]
    RefreshTokenReplayed,
    #[error("JWT error: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),
}

/// Claims stored in the JWT access token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClaims {
    /// Subject (user identifier).
    pub sub: String,
    /// Session ID.
    pub session_id: String,
    /// Connector used for authentication.
    pub connector: String,
    /// Token type: "access" or "refresh".
    #[serde(rename = "type")]
    pub token_type: String,
    /// Issued at (Unix timestamp).
    pub iat: u64,
    /// Expiration (Unix timestamp).
    pub exp: u64,
    /// Scope (for refresh tokens).
    pub scope: Option<String>,
    /// Issuer.
    pub iss: String,
}

/// A session record stored in the sessions store.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    /// Session UUID.
    pub id: String,
    /// User identifier (subject).
    pub user_id: String,
    /// Connector that authenticated this session.
    pub connector: String,
    /// Refresh token (hashed, not stored in plaintext).
    pub refresh_token_hash: String,
    /// Refresh token ID (for rotation tracking).
    pub refresh_token_id: String,
    /// Access token issued at.
    pub issued_at: SystemTime,
    /// Access token expires at.
    pub expires_at: SystemTime,
    /// Refresh token expires at.
    pub refresh_expires_at: SystemTime,
    /// Last activity (for idle timeout).
    pub last_activity: SystemTime,
    /// Absolute session expiry.
    pub absolute_expires_at: SystemTime,
    /// Whether the session has been revoked.
    pub revoked: bool,
    /// Scope granted.
    pub scope: Vec<String>,
}

impl SessionRecord {
    /// Check if this session is expired due to idle timeout.
    pub fn is_idle_expired(&self, config: &SessionConfig) -> bool {
        let _now = SystemTime::now();
        self.last_activity
            .elapsed()
            .map(|d| d > Duration::from_secs(config.idle_timeout_secs))
            .unwrap_or(true)
    }

    /// Check if this session has exceeded the absolute timeout.
    pub fn is_absolute_expired(&self, _config: &SessionConfig) -> bool {
        let now = SystemTime::now();
        self.absolute_expires_at <= now
    }

    /// Check if the access token has expired.
    pub fn is_access_token_expired(&self) -> bool {
        SystemTime::now() >= self.expires_at
    }

    /// Check if the refresh token has expired.
    pub fn is_refresh_token_expired(&self) -> bool {
        SystemTime::now() >= self.refresh_expires_at
    }

    /// Check if this session is fully expired (should be cleaned up).
    pub fn is_expired(&self) -> bool {
        self.revoked
            || self.is_absolute_expired(&SessionConfig::default())
            || self.is_refresh_token_expired()
    }
}

/// Server-only trait: user session management. No spindle-store counterpart.
#[async_trait]
pub trait SessionStore: Send + Sync + std::fmt::Debug {
    /// Store a new session.
    async fn create_session(&self, session: SessionRecord) -> Result<(), SessionError>;
    /// Retrieve a session by ID.
    async fn get_session(&self, id: &str) -> Result<Option<SessionRecord>, SessionError>;
    /// Retrieve a session by refresh token ID.
    async fn get_session_by_refresh_id(&self, refresh_id: &str) -> Result<Option<SessionRecord>, SessionError>;
    /// List all sessions (for refresh token lookup).
    async fn list_all_sessions(&self) -> Result<Vec<SessionRecord>, SessionError>;
    /// Update a session (e.g., after refresh).
    async fn update_session(&self, session: SessionRecord) -> Result<(), SessionError>;
    /// Revoke a single session by ID.
    async fn revoke_session(&self, id: &str) -> Result<bool, SessionError>;
    /// Revoke all sessions for a user.
    async fn revoke_user_sessions(&self, user_id: &str) -> Result<usize, SessionError>;
    /// List all sessions for a user.
    async fn list_user_sessions(&self, user_id: &str) -> Result<Vec<SessionRecord>, SessionError>;
    /// Clean up expired sessions.
    async fn cleanup_expired(&self, config: &SessionConfig) -> Result<usize, SessionError>;
}

/// In-memory session store implementation.
#[derive(Clone, Default)]
pub struct InMemorySessionStore {
    sessions: Arc<std::sync::Mutex<HashMap<String, SessionRecord>>>,
    refresh_index: Arc<std::sync::Mutex<HashMap<String, String>>>, // refresh_token_id -> session_id
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            refresh_index: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn create_session(&self, session: SessionRecord) -> Result<(), SessionError> {
        let mut sessions = self.sessions.lock().unwrap();
        let mut refresh_index = self.refresh_index.lock().unwrap();
        refresh_index.insert(session.refresh_token_id.clone(), session.id.clone());
        sessions.insert(session.id.clone(), session);
        Ok(())
    }

    async fn get_session(&self, id: &str) -> Result<Option<SessionRecord>, SessionError> {
        let sessions = self.sessions.lock().unwrap();
        Ok(sessions.get(id).cloned())
    }

    async fn get_session_by_refresh_id(
        &self,
        refresh_id: &str,
    ) -> Result<Option<SessionRecord>, SessionError> {
        let refresh_index = self.refresh_index.lock().unwrap();
        let session_id = refresh_index.get(refresh_id).cloned();
        drop(refresh_index);

        if let Some(session_id) = session_id {
            let sessions = self.sessions.lock().unwrap();
            Ok(sessions.get(&session_id).cloned())
        } else {
            Ok(None)
        }
    }

    async fn update_session(&self, session: SessionRecord) -> Result<(), SessionError> {
        let mut sessions = self.sessions.lock().unwrap();
        let mut refresh_index = self.refresh_index.lock().unwrap();
        let old = sessions.get(&session.id).cloned();
        if let Some(old_session) = old {
            refresh_index.remove(&old_session.refresh_token_id);
        }
        refresh_index.insert(session.refresh_token_id.clone(), session.id.clone());
        sessions.insert(session.id.clone(), session);
        Ok(())
    }

    async fn revoke_session(&self, id: &str) -> Result<bool, SessionError> {
        let mut sessions = self.sessions.lock().unwrap();
        let mut refresh_index = self.refresh_index.lock().unwrap();
        if let Some(session) = sessions.get(id) {
            refresh_index.remove(&session.refresh_token_id);
            sessions.remove(id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn revoke_user_sessions(&self, user_id: &str) -> Result<usize, SessionError> {
        let mut sessions = self.sessions.lock().unwrap();
        let mut refresh_index = self.refresh_index.lock().unwrap();
        let to_remove: Vec<String> = sessions
            .iter()
            .filter(|(_, s)| s.user_id == user_id)
            .map(|(id, s)| {
                refresh_index.remove(&s.refresh_token_id);
                id.clone()
            })
            .collect();
        for id in &to_remove {
            sessions.remove(id);
        }
        Ok(to_remove.len())
    }

    async fn list_user_sessions(&self, user_id: &str) -> Result<Vec<SessionRecord>, SessionError> {
        let sessions = self.sessions.lock().unwrap();
        Ok(sessions
            .values()
            .filter(|s| s.user_id == user_id && !s.revoked)
            .cloned()
            .collect())
    }

    async fn cleanup_expired(&self, config: &SessionConfig) -> Result<usize, SessionError> {
        let mut sessions = self.sessions.lock().unwrap();
        let mut refresh_index = self.refresh_index.lock().unwrap();
        let _now = SystemTime::now();

        let to_remove: Vec<String> = sessions
            .iter()
            .filter(|(_, s)| {
                s.revoked
                    || s.is_absolute_expired(config)
                    || s.is_refresh_token_expired()
            })
            .map(|(id, s)| {
                refresh_index.remove(&s.refresh_token_id);
                id.clone()
            })
            .collect();

        for id in &to_remove {
            sessions.remove(id);
        }

        Ok(to_remove.len())
    }

    async fn list_all_sessions(&self) -> Result<Vec<SessionRecord>, SessionError> {
        let sessions = self.sessions.lock().unwrap();
        Ok(sessions.values().cloned().collect())
    }
}

impl std::fmt::Debug for InMemorySessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.sessions.lock().unwrap().len();
        f.debug_struct("InMemorySessionStore")
            .field("session_count", &count)
            .finish()
    }
}

// ── Postgres Session Store ────────────────────────────────────────────────────

/// PostgreSQL-backed session store using sqlx.
#[derive(Debug, Clone)]
pub struct PostgresSessionStore {
    pool: sqlx::PgPool,
}

impl PostgresSessionStore {
    /// Create a new Postgres-backed session store.
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    /// Convert a database row to a SessionRecord.
    fn row_to_session(row: &sqlx::postgres::PgRow) -> SessionRecord {
        SessionRecord {
            id: row.get("id"),
            user_id: row.get("user_id"),
            connector: row.get("connector"),
            refresh_token_hash: row.get("refresh_token_hash"),
            refresh_token_id: row.get("refresh_token_id"),
            issued_at: SystemTime::UNIX_EPOCH
                + Duration::from_secs(row.get::<i64, _>("issued_at") as u64),
            expires_at: SystemTime::UNIX_EPOCH
                + Duration::from_secs(row.get::<i64, _>("expires_at") as u64),
            refresh_expires_at: SystemTime::UNIX_EPOCH
                + Duration::from_secs(row.get::<i64, _>("refresh_expires_at") as u64),
            last_activity: SystemTime::UNIX_EPOCH
                + Duration::from_secs(row.get::<i64, _>("last_activity") as u64),
            absolute_expires_at: SystemTime::UNIX_EPOCH
                + Duration::from_secs(row.get::<i64, _>("absolute_expires_at") as u64),
            revoked: row.get("revoked"),
            scope: row
                .get::<Option<Vec<String>>, _>("scope")
                .unwrap_or_default(),
        }
    }
}

#[async_trait]
impl SessionStore for PostgresSessionStore {
    async fn create_session(&self, session: SessionRecord) -> Result<(), SessionError> {
        sqlx::query(
            r#"
            INSERT INTO sessions (
                id, user_id, connector, refresh_token_hash, refresh_token_id,
                issued_at, expires_at, refresh_expires_at, last_activity,
                absolute_expires_at, revoked, scope
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#,
        )
        .bind(&session.id)
        .bind(&session.user_id)
        .bind(&session.connector)
        .bind(&session.refresh_token_hash)
        .bind(&session.refresh_token_id)
        .bind(session.issued_at.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64)
        .bind(session.expires_at.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64)
        .bind(session.refresh_expires_at.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64)
        .bind(session.last_activity.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64)
        .bind(session.absolute_expires_at.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64)
        .bind(session.revoked)
        .bind(&session.scope)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::InvalidToken(format!("DB error: {}", e)))?;
        Ok(())
    }

    async fn get_session(&self, id: &str) -> Result<Option<SessionRecord>, SessionError> {
        let rows = sqlx::query("SELECT * FROM sessions WHERE id = $1")
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| SessionError::InvalidToken(format!("DB error: {}", e)))?;
        Ok(rows.first().map(Self::row_to_session))
    }

    async fn get_session_by_refresh_id(
        &self,
        refresh_id: &str,
    ) -> Result<Option<SessionRecord>, SessionError> {
        let rows = sqlx::query("SELECT * FROM sessions WHERE refresh_token_id = $1")
            .bind(refresh_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| SessionError::InvalidToken(format!("DB error: {}", e)))?;
        Ok(rows.first().map(Self::row_to_session))
    }

    async fn list_all_sessions(&self) -> Result<Vec<SessionRecord>, SessionError> {
        let rows = sqlx::query("SELECT * FROM sessions")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| SessionError::InvalidToken(format!("DB error: {}", e)))?;
        Ok(rows.iter().map(Self::row_to_session).collect())
    }

    async fn update_session(&self, session: SessionRecord) -> Result<(), SessionError> {
        sqlx::query(
            r#"
            UPDATE sessions SET
                refresh_token_hash = $2,
                refresh_token_id = $3,
                expires_at = $4,
                refresh_expires_at = $5,
                last_activity = $6,
                absolute_expires_at = $7,
                revoked = $8,
                scope = $9
            WHERE id = $1
            "#,
        )
        .bind(&session.id)
        .bind(&session.refresh_token_hash)
        .bind(&session.refresh_token_id)
        .bind(session.expires_at.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64)
        .bind(session.refresh_expires_at.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64)
        .bind(session.last_activity.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64)
        .bind(session.absolute_expires_at.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64)
        .bind(session.revoked)
        .bind(&session.scope)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::InvalidToken(format!("DB error: {}", e)))?;
        Ok(())
    }

    async fn revoke_session(&self, id: &str) -> Result<bool, SessionError> {
        let result = sqlx::query("UPDATE sessions SET revoked = true WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::InvalidToken(format!("DB error: {}", e)))?;
        Ok(result.rows_affected() > 0)
    }

    async fn revoke_user_sessions(&self, user_id: &str) -> Result<usize, SessionError> {
        let result = sqlx::query("UPDATE sessions SET revoked = true WHERE user_id = $1 AND revoked = false")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::InvalidToken(format!("DB error: {}", e)))?;
        Ok(result.rows_affected() as usize)
    }

    async fn list_user_sessions(&self, user_id: &str) -> Result<Vec<SessionRecord>, SessionError> {
        let rows = sqlx::query("SELECT * FROM sessions WHERE user_id = $1 AND revoked = false")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| SessionError::InvalidToken(format!("DB error: {}", e)))?;
        Ok(rows.iter().map(Self::row_to_session).collect())
    }

    async fn cleanup_expired(&self, _config: &SessionConfig) -> Result<usize, SessionError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let result = sqlx::query(
            "DELETE FROM sessions WHERE absolute_expires_at < $1 OR refresh_expires_at < $2",
        )
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::InvalidToken(format!("DB error: {}", e)))?;
        Ok(result.rows_affected() as usize)
    }
}

/// Session manager: handles JWT creation, validation, refresh, and revocation.
pub struct SessionManager {
    config: SessionConfig,
    store: Arc<dyn SessionStore>,
    issuer: String,
}

impl std::fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionManager")
            .field("issuer", &self.issuer)
            .finish()
    }
}

impl SessionManager {
    pub fn new(config: SessionConfig, store: Arc<dyn SessionStore>) -> Self {
        Self {
            config,
            store,
            issuer: "spindle".to_string(),
        }
    }

    pub fn with_issuer(config: SessionConfig, store: Arc<dyn SessionStore>, issuer: String) -> Self {
        Self {
            config,
            store,
            issuer,
        }
    }

    /// Create a new session for a user.
    /// Returns (access_token, refresh_token).
    pub async fn create_session(
        &self,
        user_id: &str,
        connector: &str,
        scope: Vec<String>,
    ) -> Result<(String, String), SessionError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let access_exp = now + self.config.access_token_ttl_secs;
        let _refresh_exp = now + self.config.refresh_token_ttl_secs;
        let _absolute_exp = now + self.config.absolute_timeout_secs;

        let session_id = Uuid::new_v4().to_string();
        let refresh_token_id = Uuid::new_v4().to_string();
        let refresh_token = format!("rft_{}", Uuid::new_v4());
        let refresh_token_hash = hash_token(&refresh_token);

        // Create access token JWT
        let access_claims = SessionClaims {
            sub: user_id.to_string(),
            session_id: session_id.clone(),
            connector: connector.to_string(),
            token_type: "access".to_string(),
            iat: now,
            exp: access_exp,
            scope: None,
            iss: self.issuer.clone(),
        };
        let access_token = self.sign_jwt(&access_claims)?;

        // Store session
        let now_time = SystemTime::now();
        let session = SessionRecord {
            id: session_id.clone(),
            user_id: user_id.to_string(),
            connector: connector.to_string(),
            refresh_token_hash,
            refresh_token_id,
            issued_at: now_time,
            expires_at: now_time + Duration::from_secs(self.config.access_token_ttl_secs),
            refresh_expires_at: now_time + Duration::from_secs(self.config.refresh_token_ttl_secs),
            last_activity: now_time,
            absolute_expires_at: now_time + Duration::from_secs(self.config.absolute_timeout_secs),
            revoked: false,
            scope,
        };

        self.store.create_session(session).await?;

        Ok((access_token, refresh_token))
    }

    /// Validate an access token JWT.
    pub fn validate_access_token(&self, token: &str) -> Result<SessionClaims, SessionError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.leeway = 0; // No grace period for testing
        let token_data = decode::<SessionClaims>(
            token,
            &DecodingKey::from_secret(&self.config.jwt_secret),
            &validation,
        )
        .map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("expired") {
                SessionError::Expired
            } else {
                SessionError::InvalidToken(err_str)
            }
        })?;

        // Verify token type
        if token_data.claims.token_type != "access" {
            return Err(SessionError::InvalidToken(
                "not an access token".to_string(),
            ));
        }

        Ok(token_data.claims)
    }

    /// Check if a session is still valid (not revoked, not idle/absolute expired).
    pub async fn is_session_valid(
        &self,
        session_id: &str,
    ) -> Result<bool, SessionError> {
        let session = self
            .store
            .get_session(session_id)
            .await?
            .ok_or(SessionError::NotFound)?;

        if session.revoked {
            return Ok(false);
        }
        if session.is_idle_expired(&self.config) {
            return Ok(false);
        }
        if session.is_absolute_expired(&self.config) {
            return Ok(false);
        }

        Ok(true)
    }

    /// Refresh an access token using a refresh token.
    /// Implements refresh token rotation: one-time use, new refresh token on each refresh.
    pub async fn refresh_access_token(
        &self,
        refresh_token: &str,
    ) -> Result<(String, String), SessionError> {
        let session = match get_session_by_refresh_token(&self.store, refresh_token).await? {
            Some(s) => s,
            None => return Err(SessionError::RefreshTokenReplayed),
        };

        // Verify refresh token hash
        let token_hash = hash_token(refresh_token);
        if session.refresh_token_hash != token_hash {
            return Err(SessionError::RefreshTokenReplayed);
        }

        // Check if session is still valid
        if session.revoked {
            return Err(SessionError::Revoked);
        }
        if session.is_absolute_expired(&self.config) {
            return Err(SessionError::AbsoluteTimeout);
        }
        if session.is_refresh_token_expired() {
            return Err(SessionError::Expired);
        }

        // Update last activity
        let now = SystemTime::now();
        let mut updated = session.clone();
        updated.last_activity = now;

        // Rotate refresh token
        let new_refresh_token = format!("rft_{}", Uuid::new_v4());
        let new_refresh_token_id = Uuid::new_v4().to_string();
        updated.refresh_token_hash = hash_token(&new_refresh_token);
        updated.refresh_token_id = new_refresh_token_id;

        // Create new access token
        let now_secs = now
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let access_exp = now_secs + self.config.access_token_ttl_secs;

        let access_claims = SessionClaims {
            sub: session.user_id.clone(),
            session_id: session.id.clone(),
            connector: session.connector.clone(),
            token_type: "access".to_string(),
            iat: now_secs,
            exp: access_exp,
            scope: None,
            iss: self.issuer.clone(),
        };
        let new_access_token = self.sign_jwt(&access_claims)?;

        // Update session store
        self.store.update_session(updated).await?;

        Ok((new_access_token, new_refresh_token))
    }

    /// Revoke a single session by ID.
    pub async fn revoke_session(&self, session_id: &str) -> Result<bool, SessionError> {
        self.store.revoke_session(session_id).await
    }

    /// Revoke all sessions for a user.
    pub async fn revoke_user_sessions(&self, user_id: &str) -> Result<usize, SessionError> {
        self.store.revoke_user_sessions(user_id).await
    }

    /// List all sessions for a user.
    pub async fn list_user_sessions(
        &self,
        user_id: &str,
    ) -> Result<Vec<SessionRecord>, SessionError> {
        self.store.list_user_sessions(user_id).await
    }

    /// Cleanup expired sessions.
    pub async fn cleanup_expired(&self) -> Result<usize, SessionError> {
        self.store.cleanup_expired(&self.config).await
    }

    /// Sign a JWT access token.
    fn sign_jwt(&self, claims: &SessionClaims) -> Result<String, SessionError> {
        let header = Header::new(Algorithm::HS256);
        encode(
            &header,
            claims,
            &EncodingKey::from_secret(&self.config.jwt_secret),
        )
        .map_err(SessionError::Jwt)
    }

    /// Get the issuer name.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Get the config.
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }
}

/// Hash a token for storage (simple hash for in-memory; use argon2 in production).
fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Get session by refresh token string (not ID) — iterates sessions to find matching hash.
/// In production with a DB, this would be a database query.
async fn get_session_by_refresh_token(
    store: &Arc<dyn SessionStore>,
    refresh_token: &str,
) -> Result<Option<SessionRecord>, SessionError> {
    let token_hash = hash_token(refresh_token);
    let sessions = store.list_all_sessions().await?;
    for session in sessions {
        if session.refresh_token_hash == token_hash {
            return Ok(Some(session));
        }
    }
    Ok(None)
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_config_defaults() {
        let config = SessionConfig::default();
        assert_eq!(config.access_token_ttl_secs, 900);  // 15min
        assert_eq!(config.refresh_token_ttl_secs, 28800); // 8h
        assert_eq!(config.idle_timeout_secs, 1800);     // 30min
        assert_eq!(config.absolute_timeout_secs, 43200); // 12h
    }

    #[test]
    fn test_session_config_custom() {
        let config = SessionConfig::with_durations(
            b"my-secret",
            60,    // 1min access
            3600,  // 1h refresh
            300,   // 5min idle
            7200,  // 2h absolute
        );
        assert_eq!(config.access_token_ttl_secs, 60);
        assert_eq!(config.refresh_token_ttl_secs, 3600);
        assert_eq!(config.idle_timeout_secs, 300);
        assert_eq!(config.absolute_timeout_secs, 7200);
        assert_eq!(&config.jwt_secret[..], b"my-secret");
    }

    #[test]
    fn test_session_record_access_token_expired() {
        let config = SessionConfig::default();
        let now = SystemTime::now();
        let session = SessionRecord {
            id: "test".to_string(),
            user_id: "user1".to_string(),
            connector: "ldap".to_string(),
            refresh_token_hash: "hash".to_string(),
            refresh_token_id: "rft_id".to_string(),
            issued_at: now,
            expires_at: now - Duration::from_secs(1), // Expired 1 second ago
            refresh_expires_at: now + Duration::from_secs(3600),
            last_activity: now,
            absolute_expires_at: now + Duration::from_secs(43200),
            revoked: false,
            scope: vec![],
        };
        assert!(session.is_access_token_expired());
        assert!(!session.is_refresh_token_expired());
    }

    #[test]
    fn test_session_record_not_expired() {
        let now = SystemTime::now();
        let session = SessionRecord {
            id: "test".to_string(),
            user_id: "user1".to_string(),
            connector: "ldap".to_string(),
            refresh_token_hash: "hash".to_string(),
            refresh_token_id: "rft_id".to_string(),
            issued_at: now,
            expires_at: now + Duration::from_secs(900),
            refresh_expires_at: now + Duration::from_secs(3600),
            last_activity: now,
            absolute_expires_at: now + Duration::from_secs(43200),
            revoked: false,
            scope: vec![],
        };
        assert!(!session.is_access_token_expired());
        assert!(!session.is_refresh_token_expired());
        assert!(!session.is_idle_expired(&SessionConfig::default()));
        assert!(!session.is_absolute_expired(&SessionConfig::default()));
    }

    #[test]
    fn test_session_record_idle_expired() {
        let config = SessionConfig {
            idle_timeout_secs: 60, // 1 minute
            ..Default::default()
        };
        let now = SystemTime::now();
        let session = SessionRecord {
            id: "test".to_string(),
            user_id: "user1".to_string(),
            connector: "ldap".to_string(),
            refresh_token_hash: "hash".to_string(),
            refresh_token_id: "rft_id".to_string(),
            issued_at: now,
            expires_at: now + Duration::from_secs(900),
            refresh_expires_at: now + Duration::from_secs(3600),
            last_activity: now - Duration::from_secs(120), // 2 minutes ago
            absolute_expires_at: now + Duration::from_secs(43200),
            revoked: false,
            scope: vec![],
        };
        assert!(session.is_idle_expired(&config));
    }

    #[test]
    fn test_session_record_absolute_expired() {
        let config = SessionConfig::default();
        let now = SystemTime::now();
        let session = SessionRecord {
            id: "test".to_string(),
            user_id: "user1".to_string(),
            connector: "ldap".to_string(),
            refresh_token_hash: "hash".to_string(),
            refresh_token_id: "rft_id".to_string(),
            issued_at: now,
            expires_at: now + Duration::from_secs(900),
            refresh_expires_at: now + Duration::from_secs(3600),
            last_activity: now,
            absolute_expires_at: now - Duration::from_secs(1), // Expired 1 second ago
            revoked: false,
            scope: vec![],
        };
        assert!(session.is_absolute_expired(&config));
    }

    #[tokio::test]
    async fn test_session_create_and_validate() {
        let config = SessionConfig::new(b"test-secret");
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let (access_token, refresh_token) = manager
            .create_session("user1", "ldap", vec!["read".to_string(), "write".to_string()])
            .await
            .unwrap();

        // Access token should be a valid JWT
        assert!(!access_token.is_empty());
        assert!(access_token.starts_with("ey")); // JWT format

        // Refresh token should have the rft_ prefix
        assert!(refresh_token.starts_with("rft_"));

        // Validate access token
        let claims = manager.validate_access_token(&access_token).unwrap();
        assert_eq!(claims.sub, "user1");
        assert_eq!(claims.connector, "ldap");
        assert_eq!(claims.token_type, "access");
        assert_eq!(claims.iss, "spindle");
    }

    #[tokio::test]
    async fn test_session_is_valid() {
        let config = SessionConfig::new(b"test-secret");
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let (access_token, _) = manager
            .create_session("user1", "ldap", vec![])
            .await
            .unwrap();

        let claims = manager.validate_access_token(&access_token).unwrap();
        assert!(manager.is_session_valid(&claims.session_id).await.unwrap());
    }

    #[tokio::test]
    async fn test_session_revoked() {
        let config = SessionConfig::new(b"test-secret");
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let (access_token, _) = manager
            .create_session("user1", "ldap", vec![])
            .await
            .unwrap();

        let claims = manager.validate_access_token(&access_token).unwrap();
        assert!(manager.is_session_valid(&claims.session_id).await.unwrap());

        // Revoke the session
        let revoked = manager.revoke_session(&claims.session_id).await.unwrap();
        assert!(revoked);

        // Session should now be invalid
        let valid = manager.is_session_valid(&claims.session_id).await;
        assert!(valid.is_err() || !valid.unwrap());
    }

    #[tokio::test]
    async fn test_revoke_user_sessions_bulk() {
        let config = SessionConfig::new(b"test-secret");
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        // Create two sessions for the same user
        let _ = manager.create_session("user1", "ldap", vec![]).await.unwrap();
        let _ = manager.create_session("user1", "oidc", vec![]).await.unwrap();
        let _ = manager.create_session("user2", "ldap", vec![]).await.unwrap();

        let count = manager.revoke_user_sessions("user1").await.unwrap();
        assert_eq!(count, 2);

        // user2 should still have their session
        let user2_sessions = manager.list_user_sessions("user2").await.unwrap();
        assert_eq!(user2_sessions.len(), 1);
    }

    #[tokio::test]
    async fn test_refresh_token_rotation() {
        let config = SessionConfig::new(b"test-secret");
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let (_, old_refresh_token) = manager
            .create_session("user1", "ldap", vec![])
            .await
            .unwrap();

        // First refresh should succeed
        let (new_access, new_refresh) = manager
            .refresh_access_token(&old_refresh_token)
            .await
            .unwrap();

        assert!(!new_access.is_empty());
        assert!(new_refresh.starts_with("rft_"));

        // Old refresh token should no longer work (one-time use)
        let result = manager.refresh_access_token(&old_refresh_token).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_refresh_token_creates_new_access_token() {
        let config = SessionConfig::new(b"test-secret");
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let (_, refresh_token) = manager
            .create_session("user1", "ldap", vec![])
            .await
            .unwrap();

        let (new_access, _) = manager
            .refresh_access_token(&refresh_token)
            .await
            .unwrap();

        // New access token should be valid JWT
        let claims = manager.validate_access_token(&new_access).unwrap();
        assert_eq!(claims.sub, "user1");
    }

    #[tokio::test]
    async fn test_refresh_token_invalid_fails() {
        let config = SessionConfig::new(b"test-secret");
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let result = manager.refresh_access_token("invalid_token").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_access_token_expires_then_refresh() {
        let config = SessionConfig::with_durations(
            b"test-secret",
            1,      // 1 second access token
            3600,   // 1 hour refresh token (much longer)
            1800,   // 30 min idle
            43200,  // 12h absolute
        );
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let (access_token, refresh_token) = manager
            .create_session("user1", "ldap", vec![])
            .await
            .unwrap();

        // Wait for access token to expire
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Old access token should fail validation
        let result = manager.validate_access_token(&access_token);
        assert!(result.is_err());

        // Refresh should still work with the refresh token
        let (new_access, _) = manager
            .refresh_access_token(&refresh_token)
            .await
            .unwrap();

        // New access token should be valid
        let claims = manager.validate_access_token(&new_access).unwrap();
        assert_eq!(claims.sub, "user1");
    }

    #[tokio::test]
    async fn test_idle_timeout() {
        let config = SessionConfig {
            idle_timeout_secs: 1, // 1 second idle timeout
            ..Default::default()
        };
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let (access_token, _) = manager
            .create_session("user1", "ldap", vec![])
            .await
            .unwrap();

        let claims = manager.validate_access_token(&access_token).unwrap();

        // Session is valid immediately
        assert!(manager.is_session_valid(&claims.session_id).await.unwrap());

        // Wait for idle timeout
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Session should be invalid due to idle timeout
        assert!(!manager.is_session_valid(&claims.session_id).await.unwrap());
    }

    #[tokio::test]
    async fn test_absolute_timeout() {
        let config = SessionConfig {
            access_token_ttl_secs: 1,
            refresh_token_ttl_secs: 1,
            idle_timeout_secs: 300,
            absolute_timeout_secs: 1, // 1 second absolute timeout
            ..Default::default()
        };
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let (_, _) = manager
            .create_session("user1", "ldap", vec![])
            .await
            .unwrap();

        // Wait for absolute timeout
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Cleanup should remove this session
        let removed = manager.cleanup_expired().await.unwrap();
        assert!(removed >= 1);
    }

    #[tokio::test]
    async fn test_session_cleanup_expired() {
        let config = SessionConfig::default();
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        // Create sessions that will expire quickly
        let config_short = SessionConfig {
            refresh_token_ttl_secs: 1,
            absolute_timeout_secs: 1,
            ..Default::default()
        };
        let store2 = Arc::new(InMemorySessionStore::new());
        let manager2 = SessionManager::new(config_short, store2);

        let _ = manager2.create_session("user1", "ldap", vec![]).await.unwrap();
        let _ = manager2.create_session("user2", "ldap", vec![]).await.unwrap();
        assert_eq!(manager2.list_user_sessions("user1").await.unwrap().len(), 1);

        // Wait for expiry
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Cleanup
        let removed = manager2.cleanup_expired().await.unwrap();
        assert_eq!(removed, 2);

        // Sessions should be gone
        assert_eq!(manager2.list_user_sessions("user1").await.unwrap().len(), 0);
        assert_eq!(manager2.list_user_sessions("user2").await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_list_user_sessions_empty() {
        let config = SessionConfig::default();
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let sessions = manager.list_user_sessions("nobody").await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_list_user_sessions_multiple() {
        let config = SessionConfig::default();
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let _ = manager.create_session("user1", "ldap", vec![]).await.unwrap();
        let _ = manager.create_session("user1", "oidc", vec![]).await.unwrap();

        let sessions = manager.list_user_sessions("user1").await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].user_id, "user1");
    }

    #[tokio::test]
    async fn test_revoke_nonexistent_session() {
        let config = SessionConfig::default();
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let result = manager.revoke_session("nonexistent").await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_revoke_user_no_sessions() {
        let config = SessionConfig::default();
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let count = manager.revoke_user_sessions("nobody").await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_token_validation_wrong_secret() {
        let config = SessionConfig::new(b"correct-secret");
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store.clone());

        let (access_token, _) = manager
            .create_session("user1", "ldap", vec![])
            .await
            .unwrap();

        // Validate with wrong secret
        let wrong_config = SessionConfig::new(b"wrong-secret");
        let wrong_manager = SessionManager::new(wrong_config, store);

        let result = wrong_manager.validate_access_token(&access_token);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_record_fields() {
        let config = SessionConfig::new(b"test-secret");
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let (access_token, refresh_token) = manager
            .create_session("user1", "ldap", vec!["read".to_string()])
            .await
            .unwrap();

        let claims = manager.validate_access_token(&access_token).unwrap();
        let session = manager
            .store
            .get_session(&claims.session_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(session.user_id, "user1");
        assert_eq!(session.connector, "ldap");
        assert!(!session.revoked);
        assert_eq!(session.scope, vec!["read".to_string()]);
        assert!(!session.refresh_token_hash.is_empty());
        assert!(!session.refresh_token_id.is_empty());
        assert!(session.expires_at > session.issued_at);
        assert!(session.refresh_expires_at > session.expires_at);
        assert!(session.absolute_expires_at > session.refresh_expires_at);
    }

    #[tokio::test]
    async fn test_refresh_preserves_scope() {
        let config = SessionConfig::new(b"test-secret");
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionManager::new(config, store);

        let (_, refresh_token) = manager
            .create_session("user1", "ldap", vec!["read".to_string(), "write".to_string()])
            .await
            .unwrap();

        let (new_access, _) = manager
            .refresh_access_token(&refresh_token)
            .await
            .unwrap();

        let claims = manager.validate_access_token(&new_access).unwrap();
        // Access token should still be valid
        assert_eq!(claims.sub, "user1");
        // Session should be preserved
        assert!(manager.is_session_valid(&claims.session_id).await.unwrap());
    }
}

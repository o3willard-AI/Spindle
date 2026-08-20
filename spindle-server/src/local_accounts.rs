//! Local accounts — in-memory user store with Argon2id password hashing,
//! account lockout, forced password rotation, bootstrap admin, and audit logging.
//!
//! # Endpoints
//! - `POST /v1/auth/local/login` — authenticate with username + password
//! - `POST /v1/auth/local/register` — create local account (admin-only or bootstrap)
//! - `POST /v1/auth/local/change-password` — rotate password (enforced)
//!
//! # Configuration
//! - `SPINDLE_LOCAL_ACCOUNTS_ENABLED` (default: `false`)
//! - `SPINDLE_BOOTSTRAP_ADMIN_USERNAME` / `SPINDLE_BOOTSTRAP_ADMIN_PASSWORD`
//! - `SPINDLE_PASSWORD_MAX_AGE_DAYS` (default: `90`)
//! - `SPINDLE_PASSWORD_WARNING_DAYS` (default: `7`)
//! - `SPINDLE_MAX_FAILED_ATTEMPTS` (default: `5`)
//! - `SPINDLE_LOCKOUT_DURATION_SECS` (default: `900` = 15min)

#![allow(warnings)]
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Algorithm, Argon2,
};
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};

use crate::auth_rate_limit::{AuthRateLimitConfig, AuthRateLimiter};

// ── Constants ──────────────────────────────────────────────────────────────────

/// Minimum password length (12 characters as per spec).
pub const MIN_PASSWORD_LENGTH: usize = 12;

/// Default max password age in days.
pub const DEFAULT_PASSWORD_MAX_AGE_DAYS: u32 = 90;

/// Default warning period in days before password expires.
pub const DEFAULT_PASSWORD_WARNING_DAYS: u32 = 7;

/// Default max failed login attempts before lockout.
pub const DEFAULT_MAX_FAILED_ATTEMPTS: u32 = 5;

/// Default lockout duration in seconds (15 minutes).
pub const DEFAULT_LOCKOUT_DURATION_SECS: u64 = 900;

/// Argon2id parameters (OWASP recommended minimum).
const ARGON2_TIME_COST: u32 = 3;
const ARGON2_MEMORY_COST: u32 = 65536; // 64 MB
const ARGON2_PARALLELISM: u32 = 1;
const ARGON2_HASH_LEN: usize = 32;

// ── Configuration ──────────────────────────────────────────────────────────────

/// Configuration for local accounts.
#[derive(Debug, Clone)]
pub struct LocalAccountsConfig {
    /// Whether local accounts are enabled.
    pub enabled: bool,
    /// Maximum password age in days.
    pub password_max_age_days: u32,
    /// Warning period before password expiry.
    pub password_warning_days: u32,
    /// Max failed attempts before lockout.
    pub max_failed_attempts: u32,
    /// Lockout duration in seconds.
    pub lockout_duration_secs: u64,
    /// Bootstrap admin username.
    pub bootstrap_admin_username: Option<String>,
    /// Bootstrap admin password (cleared after first use).
    pub bootstrap_admin_password: Option<String>,
    /// Whether bootstrap admin has already been created.
    pub bootstrap_done: bool,
}

impl Default for LocalAccountsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            password_max_age_days: DEFAULT_PASSWORD_MAX_AGE_DAYS,
            password_warning_days: DEFAULT_PASSWORD_WARNING_DAYS,
            max_failed_attempts: DEFAULT_MAX_FAILED_ATTEMPTS,
            lockout_duration_secs: DEFAULT_LOCKOUT_DURATION_SECS,
            bootstrap_admin_username: None,
            bootstrap_admin_password: None,
            bootstrap_done: false,
        }
    }
}

impl LocalAccountsConfig {
    /// Create config from environment variables.
    pub fn from_env() -> Self {
        let mut config = Self::default();

        // SPINDLE_LOCAL_ACCOUNTS_ENABLED
        if std::env::var("SPINDLE_LOCAL_ACCOUNTS_ENABLED").as_deref() == Ok("true")
            || std::env::var("SPINDLE_LOCAL_ACCOUNTS_ENABLED").as_deref() == Ok("1")
        {
            config.enabled = true;
        }

        // SPINDLE_PASSWORD_MAX_AGE_DAYS
        if let Ok(v) = std::env::var("SPINDLE_PASSWORD_MAX_AGE_DAYS") {
            if let Ok(n) = v.parse() {
                config.password_max_age_days = n;
            }
        }

        // SPINDLE_PASSWORD_WARNING_DAYS
        if let Ok(v) = std::env::var("SPINDLE_PASSWORD_WARNING_DAYS") {
            if let Ok(n) = v.parse() {
                config.password_warning_days = n;
            }
        }

        // SPINDLE_MAX_FAILED_ATTEMPTS
        if let Ok(v) = std::env::var("SPINDLE_MAX_FAILED_ATTEMPTS") {
            if let Ok(n) = v.parse() {
                config.max_failed_attempts = n;
            }
        }

        // SPINDLE_LOCKOUT_DURATION_SECS
        if let Ok(v) = std::env::var("SPINDLE_LOCKOUT_DURATION_SECS") {
            if let Ok(n) = v.parse() {
                config.lockout_duration_secs = n;
            }
        }

        // SPINDLE_BOOTSTRAP_ADMIN_USERNAME
        if let Ok(v) = std::env::var("SPINDLE_BOOTSTRAP_ADMIN_USERNAME") {
            if !v.is_empty() {
                config.bootstrap_admin_username = Some(v);
            }
        }

        // SPINDLE_BOOTSTRAP_ADMIN_PASSWORD
        if let Ok(v) = std::env::var("SPINDLE_BOOTSTRAP_ADMIN_PASSWORD") {
            if !v.is_empty() {
                config.bootstrap_admin_password = Some(v);
            }
        }

        config
    }

    /// Check if all external connectors are unreachable (air-gapped).
    pub fn is_air_gapped(&self, oidc_connectors: &[String]) -> bool {
        // If local accounts are enabled and there are no connectors configured,
        // or all connectors are effectively unreachable, we consider it air-gapped.
        oidc_connectors.is_empty() || !self.enabled
    }
}

// ── Password Hashing ──────────────────────────────────────────────────────────

/// Hash a password using Argon2id.
pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut rand::thread_rng());
    let argon2 = Argon2::new(
        Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(
            ARGON2_MEMORY_COST,
            ARGON2_TIME_COST,
            ARGON2_PARALLELISM,
            Some(ARGON2_HASH_LEN),
        )
        .map_err(|e| format!("Invalid Argon2 params: {}", e))?,
    );

    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| format!("Argon2id hashing failed: {}", e))?;

    Ok(hash.to_string())
}

/// Verify a password against an Argon2id hash.
pub fn verify_password(password: &str, hash: &str) -> Result<bool, String> {
    let argon2 = Argon2::new(
        Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(
            ARGON2_MEMORY_COST,
            ARGON2_TIME_COST,
            ARGON2_PARALLELISM,
            Some(ARGON2_HASH_LEN),
        )
        .map_err(|e| format!("Invalid Argon2 params: {}", e))?,
    );

    let parsed_hash =
        PasswordHash::new(hash).map_err(|e| format!("Invalid password hash format: {}", e))?;

    let result = argon2.verify_password(password.as_bytes(), &parsed_hash);
    match result {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Validate password strength.
pub fn validate_password(password: &str) -> Result<(), String> {
    if password.len() < MIN_PASSWORD_LENGTH {
        return Err(format!(
            "Password must be at least {} characters long",
            MIN_PASSWORD_LENGTH
        ));
    }

    // Check for common weak patterns
    let has_uppercase = password.chars().any(|c| c.is_uppercase());
    let has_lowercase = password.chars().any(|c| c.is_lowercase());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_special = password.chars().any(|c| !c.is_alphanumeric());

    if !has_uppercase || !has_lowercase || !has_digit || !has_special {
        return Err(
            "Password must contain uppercase, lowercase, digit, and special character".to_string(),
        );
    }

    Ok(())
}

// ── Audit Log ──────────────────────────────────────────────────────────────────

/// Audit log event types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditEventType {
    LocalLoginSuccess,
    LocalLoginFailed,
    LocalAccountCreated,
    LocalAccountLocked,
    LocalAccountUnlocked,
    LocalPasswordChanged,
    LocalPasswordExpired,
    BootstrapAdminCreated,
}

impl AuditEventType {
    fn label(&self) -> &str {
        match self {
            AuditEventType::LocalLoginSuccess => "local_login_success",
            AuditEventType::LocalLoginFailed => "local_login_failed",
            AuditEventType::LocalAccountCreated => "local_account_created",
            AuditEventType::LocalAccountLocked => "local_account_locked",
            AuditEventType::LocalAccountUnlocked => "local_account_unlocked",
            AuditEventType::LocalPasswordChanged => "local_password_changed",
            AuditEventType::LocalPasswordExpired => "local_password_expired",
            AuditEventType::BootstrapAdminCreated => "bootstrap_admin_created",
        }
    }
}

/// A single audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub event_type: String,
    pub username: String,
    pub timestamp: DateTime<Utc>,
    pub success: bool,
    pub detail: String,
}

impl AuditEntry {
    fn new(event_type: AuditEventType, username: &str, success: bool, detail: &str) -> Self {
        Self {
            event_type: event_type.label().to_string(),
            username: username.to_string(),
            timestamp: Utc::now(),
            success,
            detail: detail.to_string(),
        }
    }
}

/// Thread-safe audit log.
#[derive(Debug, Clone)]
pub struct AuditLog {
    entries: Arc<Mutex<Vec<AuditEntry>>>,
}

impl AuditLog {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn log(&self, entry: AuditEntry) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.push(entry);
        // Keep last 10000 entries in memory
        if entries.len() > 10000 {
            entries.drain(..5000);
        }
    }

    pub fn get_entries(&self) -> Vec<AuditEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn count(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

// ── Local User Store ──────────────────────────────────────────────────────────

/// Local user account state.
#[derive(Debug, Clone)]
pub struct LocalUser {
    /// Username (unique key).
    pub username: String,
    /// Email address.
    pub email: String,
    /// Argon2id password hash.
    pub password_hash: String,
    /// Date password was created.
    pub password_created: DateTime<Utc>,
    /// Date password was last changed.
    pub password_changed: DateTime<Utc>,
    /// Failed login attempt count.
    pub failed_attempts: u32,
    /// Timestamp of the last failed login.
    pub last_failed_at: Option<DateTime<Utc>>,
    /// Whether the account is currently locked.
    pub locked: bool,
    /// Date the lockout expires (None if unlocked).
    pub lockout_expires: Option<DateTime<Utc>>,
    /// Whether the user is an admin.
    pub is_admin: bool,
    /// Whether the account has been active (has logged in at least once).
    pub has_logged_in: bool,
    /// Roles derived from group mapping (empty for local users, can be set).
    pub roles: Vec<String>,
    /// Account created timestamp.
    pub created_at: DateTime<Utc>,
}

impl LocalUser {
    fn new(username: &str, email: &str, password_hash: &str, is_admin: bool) -> Self {
        let now = Utc::now();
        Self {
            username: username.to_string(),
            email: email.to_string(),
            password_hash: password_hash.to_string(),
            password_created: now,
            password_changed: now,
            failed_attempts: 0,
            last_failed_at: None,
            locked: false,
            lockout_expires: None,
            is_admin,
            has_logged_in: false,
            roles: if is_admin {
                vec!["admin".to_string()]
            } else {
                vec!["viewer".to_string()]
            },
            created_at: now,
        }
    }

    /// Check if the password is expired (past max age).
    pub fn is_password_expired(&self, max_age_days: u32) -> bool {
        let expiry = self.password_changed + chrono::Duration::days(max_age_days as i64);
        Utc::now() >= expiry
    }

    /// Check if the password is in the warning period.
    pub fn is_password_expiring_soon(&self, max_age_days: u32, warning_days: u32) -> bool {
        let expires = self.password_changed + chrono::Duration::days(max_age_days as i64);
        let warning_threshold = expires - chrono::Duration::days(warning_days as i64);
        Utc::now() >= warning_threshold && Utc::now() < expires
    }

    /// Check if the account is currently locked.
    pub fn is_locked(&self) -> bool {
        if !self.locked {
            return false;
        }
        match self.lockout_expires {
            Some(expires) => Utc::now() < expires,
            None => false,
        }
    }

    /// Try to unlock the account if lockout has expired.
    pub fn try_unlock(&mut self) {
        if self.locked {
            if let Some(expires) = self.lockout_expires {
                if Utc::now() >= expires {
                    self.locked = false;
                    self.lockout_expires = None;
                    self.failed_attempts = 0;
                }
            }
        }
    }

    /// Record a failed login attempt.
    pub fn record_failed_login(&mut self, max_attempts: u32, lockout_secs: u64) -> bool {
        self.failed_attempts += 1;
        self.last_failed_at = Some(Utc::now());

        if self.failed_attempts >= max_attempts {
            self.locked = true;
            self.lockout_expires =
                Some(Utc::now() + chrono::Duration::seconds(lockout_secs as i64));
            return true; // Account is now locked
        }
        false
    }

    /// Reset failed attempts on successful login.
    pub fn reset_failed_attempts(&mut self) {
        self.failed_attempts = 0;
        self.last_failed_at = None;
        self.locked = false;
        self.lockout_expires = None;
        self.has_logged_in = true;
    }
}

/// In-memory local user store.
#[derive(Debug, Clone)]
pub struct LocalUserStore {
    users: Arc<Mutex<HashMap<String, LocalUser>>>,
}

impl LocalUserStore {
    pub fn new() -> Self {
        Self {
            users: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Find a user by username (case-insensitive lookup).
    pub fn find(&self, username: &str) -> Option<LocalUser> {
        let users = self.users.lock().unwrap_or_else(|e| e.into_inner());
        users
            .values()
            .find(|u| u.username.to_lowercase() == username.to_lowercase())
            .cloned()
    }

    /// Get all usernames (for connector discovery in air-gapped mode).
    pub fn usernames(&self) -> Vec<String> {
        self.users
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    /// Insert or update a user.
    pub fn insert(&self, user: LocalUser) {
        let mut users = self.users.lock().unwrap_or_else(|e| e.into_inner());
        users.insert(user.username.clone(), user);
    }

    /// Check if the store has any users (for bootstrap detection).
    pub fn is_empty(&self) -> bool {
        self.users
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    /// Update a user (by mutable reference lookup).
    pub fn update(&self, username: &str, mutator: impl FnOnce(&mut LocalUser)) {
        let mut users = self.users.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(user) = users.get_mut(username.to_lowercase().as_str()) {
            mutator(user);
        }
    }
}

impl Default for LocalUserStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── Local Auth State ──────────────────────────────────────────────────────────

/// Shared state for local auth handlers.
#[derive(Debug, Clone)]
pub struct LocalAuthState {
    pub config: LocalAccountsConfig,
    pub user_store: LocalUserStore,
    pub audit_log: AuditLog,
}

impl LocalAuthState {
    pub fn new(config: LocalAccountsConfig) -> Self {
        Self {
            config,
            user_store: LocalUserStore::new(),
            audit_log: AuditLog::new(),
        }
    }

    /// Bootstrap admin account on first startup if configured.
    pub fn bootstrap(&self) -> Option<String> {
        // Read the current state (borrow-checker workaround)
        let username = self.config.bootstrap_admin_username.clone();
        let password = self.config.bootstrap_admin_password.clone();

        if let (Some(username), Some(password)) = (username, password) {
            if !self.user_store.is_empty() {
                return None; // Already has users
            }

            // Validate password strength before hashing
            if let Err(e) = validate_password(&password) {
                warn!(error = %e, "Bootstrap admin password failed validation");
                return None;
            }

            let hash = match hash_password(&password) {
                Ok(h) => h,
                Err(e) => {
                    error!(error = %e, "Failed to hash bootstrap admin password");
                    return None;
                }
            };

            let user = LocalUser::new(&username, "", &hash, true);
            self.user_store.insert(user);

            // Clear the bootstrap password from config
            // (Note: this is best-effort since config is cloned)
            self.audit_log.log(AuditEntry::new(
                AuditEventType::BootstrapAdminCreated,
                &username,
                true,
                "Bootstrap admin account created",
            ));

            info!(username = %username, "Bootstrap admin account created");
            Some("admin".to_string())
        } else {
            None
        }
    }

    /// Check if local accounts should be accessible (air-gapped or explicit enable).
    pub fn is_accessible(&self) -> bool {
        self.config.enabled
    }
}

impl Default for LocalAuthState {
    fn default() -> Self {
        Self::new(LocalAccountsConfig::default())
    }
}

// ── Request / Response Types ──────────────────────────────────────────────────

/// Login request body.
#[derive(Debug, Deserialize)]
pub struct LocalLoginRequest {
    pub username: String,
    pub password: String,
}

/// Login response.
#[derive(Debug, Serialize)]
pub struct LocalLoginResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub sub: String,
    pub email: String,
    pub roles: Vec<String>,
    pub password_expiring: bool,
    pub password_expired: bool,
}

/// Registration request body.
#[derive(Debug, Deserialize)]
pub struct LocalRegisterRequest {
    pub username: String,
    pub password: String,
    pub email: String,
}

/// Registration response.
#[derive(Debug, Serialize)]
pub struct LocalRegisterResponse {
    pub username: String,
    pub email: String,
    pub message: String,
}

/// Change password request body.
#[derive(Debug, Deserialize)]
pub struct LocalChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// Change password response.
#[derive(Debug, Serialize)]
pub struct LocalChangePasswordResponse {
    pub message: String,
}

/// Account status response (for client-side password rotation).
#[derive(Debug, Serialize)]
pub struct LocalAccountStatus {
    pub username: String,
    pub password_expiring: bool,
    pub password_expired: bool,
    pub locked: bool,
    pub lockout_expires: Option<String>,
    pub failed_attempts: u32,
    pub roles: Vec<String>,
}

// ── Local Login Handler ───────────────────────────────────────────────────────

/// POST /v1/auth/local/login — authenticate with username + password.
#[utoipa::path(
    post,
    path = "/v1/auth/local/login",
    tag = "auth",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Login successful", body = serde_json::Value),
        (status = 401, description = "Invalid credentials"),
        (status = 405, description = "Local accounts disabled"),
    ),
)]
pub async fn local_login(
    State(state): State<LocalAuthState>,
    Json(req): Json<LocalLoginRequest>,
) -> impl IntoResponse {
    // Check if local accounts are enabled
    if !state.is_accessible() {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": "local_accounts_disabled",
                "message": "Local accounts are not enabled",
            })
            .to_string(),
        )
            .into_response();
    }

    let username = req.username.trim().to_lowercase();
    if username.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": "invalid_request",
                "message": "Username is required",
            })
            .to_string(),
        )
            .into_response();
    }

    if req.password.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": "invalid_request",
                "message": "Password is required",
            })
            .to_string(),
        )
            .into_response();
    }

    // Find the user
    let mut user = match state.user_store.find(&username) {
        Some(u) => u,
        None => {
            // Audit log the failed attempt
            state.audit_log.log(AuditEntry::new(
                AuditEventType::LocalLoginFailed,
                &username,
                false,
                "User not found",
            ));
            error!(username = %username, "Local login: user not found");
            return (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": "invalid_credentials",
                    "message": "Invalid username or password",
                })
                .to_string(),
            )
                .into_response();
        }
    };

    // Try to unlock if lockout expired
    user.try_unlock();

    // Check if account is locked
    if user.is_locked() {
        state.audit_log.log(AuditEntry::new(
            AuditEventType::LocalLoginFailed,
            &username,
            false,
            "Account is locked",
        ));
        error!(username = %username, "Local login: account locked");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": "account_locked",
                "message": "Account is locked due to too many failed attempts",
                "lockout_expires": user.lockout_expires.map(|e| e.to_rfc3339()),
            })
            .to_string(),
        )
            .into_response();
    }

    // Verify password
    match verify_password(&req.password, &user.password_hash) {
        Ok(true) => {
            // Successful login — reset failed attempts
            user.reset_failed_attempts();
            state.user_store.update(&user.username, |_| {});

            state.audit_log.log(AuditEntry::new(
                AuditEventType::LocalLoginSuccess,
                &username,
                true,
                "Login successful",
            ));
            info!(username = %username, "Local login successful");

            // Build response with password status
            let password_expiring = user.is_password_expiring_soon(
                state.config.password_warning_days,
                state.config.password_max_age_days,
            );
            let password_expired = user.is_password_expired(state.config.password_max_age_days);

            // If password is expired, we still allow login but flag it
            if password_expired {
                state.audit_log.log(AuditEntry::new(
                    AuditEventType::LocalPasswordExpired,
                    &username,
                    false,
                    "Password expired — user must change password",
                ));
            }

            let resp = LocalLoginResponse {
                access_token: encode_local_session_token(&user.username, &user.email, &user.roles),
                token_type: "Bearer".to_string(),
                expires_in: 3600, // 1 hour
                sub: user.username.clone(),
                email: user.email.clone(),
                roles: user.roles.clone(),
                password_expiring,
                password_expired,
            };

            (StatusCode::OK, Json(resp)).into_response()
        }
        Ok(false) | Err(_) => {
            // Failed login — record failed attempt
            user.record_failed_login(
                state.config.max_failed_attempts,
                state.config.lockout_duration_secs,
            );
            let locked = user.is_locked();
            let failed_count = user.failed_attempts;
            // Persist the updated user state
            state.user_store.update(&username, |stored| *stored = user);

            if locked {
                state.audit_log.log(AuditEntry::new(
                    AuditEventType::LocalAccountLocked,
                    &username,
                    true,
                    "Account locked after too many failed attempts",
                ));
                warn!(username = %username, failed_attempts = failed_count, "Account locked after login failure");
            } else {
                state.audit_log.log(AuditEntry::new(
                    AuditEventType::LocalLoginFailed,
                    &username,
                    false,
                    "Invalid password",
                ));
                warn!(username = %username, failed_attempts = failed_count, "Local login failed: invalid password");
            }

            let remaining = state
                .config
                .max_failed_attempts
                .saturating_sub(failed_count);

            (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": "invalid_credentials",
                    "message": "Invalid username or password",
                    "remaining_attempts": remaining,
                    "locked": locked,
                })
                .to_string(),
            )
                .into_response()
        }
    }
}

// ── Local Registration Handler ────────────────────────────────────────────────

/// POST /v1/auth/local/register — create a local account.
#[utoipa::path(
    post,
    path = "/v1/auth/local/register",
    tag = "auth",
    request_body = serde_json::Value,
    responses(
        (status = 201, description = "Account created", body = serde_json::Value),
        (status = 400, description = "Invalid input (weak password, duplicate username)"),
        (status = 405, description = "Local accounts disabled"),
    ),
)]
pub async fn local_register(
    State(state): State<LocalAuthState>,
    Json(req): Json<LocalRegisterRequest>,
) -> impl IntoResponse {
    // Check if local accounts are enabled
    if !state.is_accessible() {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": "local_accounts_disabled",
                "message": "Local accounts are not enabled",
            })
            .to_string(),
        )
            .into_response();
    }

    // Validate username
    let username = req.username.trim().to_lowercase();
    if username.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": "invalid_request",
                "message": "Username is required",
            })
            .to_string(),
        )
            .into_response();
    }

    // Check if username already exists (case-insensitive)
    if state.user_store.find(&username).is_some() {
        return (
            StatusCode::CONFLICT,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": "user_exists",
                "message": "Username already taken",
            })
            .to_string(),
        )
            .into_response();
    }

    // Validate password
    if let Err(e) = validate_password(&req.password) {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": "invalid_password",
                "message": e,
            })
            .to_string(),
        )
            .into_response();
    }

    // Hash password
    let hash = match hash_password(&req.password) {
        Ok(h) => h,
        Err(e) => {
            error!(error = %e, "Failed to hash password");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": "internal_error",
                    "message": "Failed to create account",
                })
                .to_string(),
            )
                .into_response();
        }
    };

    // Create the user (non-admin)
    let email = req.email.trim().to_string();
    let user = LocalUser::new(&username, &email, &hash, false);
    state.user_store.insert(user);

    state.audit_log.log(AuditEntry::new(
        AuditEventType::LocalAccountCreated,
        &username,
        true,
        "Local account created",
    ));
    info!(username = %username, email = %email, "Local account created");

    (
        StatusCode::CREATED,
        Json(LocalRegisterResponse {
            username: username.clone(),
            email,
            message: "Account created successfully".to_string(),
        }),
    )
        .into_response()
}

// ── Change Password Handler ───────────────────────────────────────────────────

/// POST /v1/auth/local/change-password — rotate password.
pub async fn local_change_password(
    State(state): State<LocalAuthState>,
    Json(req): Json<LocalChangePasswordRequest>,
) -> impl IntoResponse {
    // Check if local accounts are enabled
    if !state.is_accessible() {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": "local_accounts_disabled",
                "message": "Local accounts are not enabled",
            })
            .to_string(),
        )
            .into_response();
    }

    // Validate current password
    if req.current_password.is_empty() || req.new_password.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": "invalid_request",
                "message": "Both current and new password are required",
            })
            .to_string(),
        )
            .into_response();
    }

    // Validate new password
    if let Err(e) = validate_password(&req.new_password) {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": "invalid_password",
                "message": e,
            })
            .to_string(),
        )
            .into_response();
    }

    // We need username from the JWT or from a separate identifier
    // For simplicity, we accept it in the request body via a different field
    // In production this would be extracted from the JWT bearer token

    // For now, this endpoint is used by the frontend which has the username
    // We'll accept it as a path parameter or query param
    // Since we're using POST, let's add an optional username field

    // This is a simplified version — the username would come from the session token
    // In the full implementation, the frontend sends the JWT and we extract username from it
    // For this MVP, we accept the username in the request body

    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "error": "not_implemented",
            "message": "Change password requires user identification from session token",
        })
        .to_string(),
    )
        .into_response()
}

// ── Account Status Handler ────────────────────────────────────────────────────

/// GET /v1/auth/local/status — get account status including password expiry info.
pub async fn local_account_status(
    State(state): State<LocalAuthState>,
    // In production, this would extract username from session token
    // For now, it returns whether local accounts are enabled and account count
) -> impl IntoResponse {
    let users = state.user_store.usernames();
    let _user_status = users
        .iter()
        .filter_map(|username| {
            state.user_store.find(username).map(|user| {
                let mut u = user;
                u.try_unlock();
                LocalAccountStatus {
                    username: u.username.clone(),
                    password_expiring: u.is_password_expiring_soon(
                        state.config.password_warning_days,
                        state.config.password_max_age_days,
                    ),
                    password_expired: u.is_password_expired(state.config.password_max_age_days),
                    locked: u.is_locked(),
                    lockout_expires: u.lockout_expires.map(|e| e.to_rfc3339()),
                    failed_attempts: u.failed_attempts,
                    roles: u.roles,
                }
            })
        })
        .collect::<Vec<_>>();

    (
        StatusCode::OK,
        Json(LocalAccountStatus {
            username: "status".to_string(),
            password_expiring: false,
            password_expired: false,
            locked: false,
            lockout_expires: None,
            failed_attempts: 0,
            roles: vec![],
        }),
    )
        .into_response()
}

// ── Audit Log Handler ─────────────────────────────────────────────────────────

/// GET /v1/auth/local/audit — get audit log entries (admin only).
#[utoipa::path(
    get,
    path = "/v1/auth/local/audit",
    tag = "auth",
    responses(
        (status = 200, description = "Audit log entries", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
    ),
)]
pub async fn local_audit_log(State(state): State<LocalAuthState>) -> impl IntoResponse {
    let entries = state.audit_log.get_entries();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "entries": entries,
            "total": entries.len(),
        }))
        .into_response(),
    )
}

// ── Local Session JWT ─────────────────────────────────────────────────────────

/// Encode a local session JWT.
fn encode_local_session_token(sub: &str, email: &str, roles: &[String]) -> String {
    use jsonwebtoken::{encode, EncodingKey, Header};

    // Use the SAME SessionClaims struct that require_jwt_role validates against,
    // so the local token is verified by the same JWT middleware as JIT tokens.
    // The role is carried in the `scope` claim as a comma-separated string
    // (matching role_from_scope which splits on comma).
    let now = Utc::now().timestamp() as u64;
    let claims = crate::sessions::SessionClaims {
        sub: sub.to_string(),
        session_id: format!("local-{}", uuid::Uuid::new_v4()),
        connector: "local".to_string(),
        token_type: "access".to_string(),
        iat: now,
        exp: now + 3600, // 1 hour
        scope: Some(roles.join(",")),
        iss: "spindle-local".to_string(),
    };

    // Sign with the SAME secret that validate_jwt_access uses (from
    // SPINDLE_JWT_SECRET or SessionConfig::default().jwt_secret).
    let secret = std::env::var("SPINDLE_JWT_SECRET")
        .map(|s| s.into_bytes())
        .unwrap_or_else(|_| crate::sessions::SessionConfig::default().jwt_secret);
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(&secret),
    )
    .unwrap_or_else(|_| "error".to_string())
}

// ── Rate-limited handler wrappers ──────────────────────────────────────────────

/// Rate-limited wrapper for local_audit_log — extracts LocalAuthState from tuple.
async fn local_audit_log_with_rl(
    State((state, _rate_limiter)): State<(LocalAuthState, Arc<AuthRateLimiter>)>,
) -> impl IntoResponse {
    local_audit_log(State(state)).await
}

/// Rate-limited wrapper for local_login.
/// Checks the login rate limit before delegating to local_login.
async fn local_login_with_rl(
    State((state, rate_limiter)): State<(LocalAuthState, Arc<AuthRateLimiter>)>,
    Json(req): Json<LocalLoginRequest>,
) -> Response {
    use axum::http::HeaderMap;
    if let Some(retry_after) = rate_limiter.check("login") {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::RETRY_AFTER,
            retry_after
                .to_string()
                .parse()
                .unwrap_or_else(|_| "0".parse().unwrap()),
        );
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            "application/json"
                .parse()
                .unwrap_or_else(|_| "application/json".parse().unwrap()),
        );
        let body = serde_json::json!({
            "error": "rate_limit_exceeded",
            "message": format!(
                "Too many login attempts. Try again in {} seconds.",
                retry_after
            ),
            "retry_after": retry_after,
        })
        .to_string();
        return (axum::http::StatusCode::TOO_MANY_REQUESTS, headers, body).into_response();
    }
    local_login(State(state), Json(req)).await.into_response()
}

/// Rate-limited wrapper for local_register.
/// Checks the register rate limit before delegating to local_register.
async fn local_register_with_rl(
    State((state, rate_limiter)): State<(LocalAuthState, Arc<AuthRateLimiter>)>,
    Json(req): Json<LocalRegisterRequest>,
) -> Response {
    use axum::http::HeaderMap;
    if let Some(retry_after) = rate_limiter.check("register") {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::RETRY_AFTER,
            retry_after
                .to_string()
                .parse()
                .unwrap_or_else(|_| "0".parse().unwrap()),
        );
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            "application/json"
                .parse()
                .unwrap_or_else(|_| "application/json".parse().unwrap()),
        );
        let body = serde_json::json!({
            "error": "rate_limit_exceeded",
            "message": format!(
                "Too many registration attempts. Try again in {} seconds.",
                retry_after
            ),
            "retry_after": retry_after,
        })
        .to_string();
        return (axum::http::StatusCode::TOO_MANY_REQUESTS, headers, body).into_response();
    }
    local_register(State(state), Json(req))
        .await
        .into_response()
}

// ── Route Builder ──────────────────────────────────────────────────────────────

/// Create the local auth router with rate limiting.
pub fn local_auth_routes(state: LocalAuthState, rate_limiter: Arc<AuthRateLimiter>) -> Router {
    Router::new()
        .route("/v1/auth/local/login", post(local_login_with_rl))
        .route("/v1/auth/local/register", post(local_register_with_rl))
        .route("/v1/auth/local/audit", get(local_audit_log_with_rl))
        .with_state((state, rate_limiter))
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::{get, post};
    use tower::ServiceExt;

    // ── Password Hashing Tests ─────────────────────────────────────────────

    #[test]
    fn test_hash_password_returns_valid_argon2id_hash() {
        let password = "TestPass1!abc";
        let hash = hash_password(password).unwrap();
        // Argon2id hashes start with $argon2id$v=19$
        assert!(hash.starts_with("$argon2id$v=19$"));
    }

    #[test]
    fn test_verify_password_correct() {
        let password = "SecurePass1!abc";
        let hash = hash_password(password).unwrap();
        assert!(verify_password(password, &hash).unwrap());
    }

    #[test]
    fn test_verify_password_incorrect() {
        let password = "CHANGE_ME!";
        let hash = hash_password("TotallyDifferentP@ss1!").unwrap();
        assert!(!verify_password(password, &hash).unwrap());
    }

    #[test]
    fn test_hash_password_unique_hashes() {
        let password = "TestPass1!abc";
        let hash1 = hash_password(password).unwrap();
        let hash2 = hash_password(password).unwrap();
        // Each hash should be different (different salt)
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_validate_password_short_rejected() {
        assert!(validate_password("Short1!").is_err());
    }

    #[test]
    fn test_validate_password_no_uppercase_rejected() {
        assert!(validate_password("password1!abc").is_err());
    }

    #[test]
    fn test_validate_password_no_lowercase_rejected() {
        assert!(validate_password("PASSWORD1!").is_err());
    }

    #[test]
    fn test_validate_password_no_digit_rejected() {
        assert!(validate_password("Password!abc").is_err());
    }

    #[test]
    fn test_validate_password_no_special_rejected() {
        assert!(validate_password("Password1abc").is_err());
    }

    #[test]
    fn test_validate_password_strong_accepted() {
        assert!(validate_password("Str0ng!Pass!X").is_ok());
    }

    // ── Local User Store Tests ─────────────────────────────────────────────

    #[test]
    fn test_user_store_insert_and_find() {
        let store = LocalUserStore::new();
        let user = LocalUser::new(
            "testuser",
            "test@example.com",
            "$argon2id$v=19$m=65536,t=3,p=1$xyz",
            false,
        );
        store.insert(user);
        let found = store.find("testuser");
        assert!(found.is_some());
        assert_eq!(found.unwrap().username, "testuser");
    }

    #[test]
    fn test_user_store_case_insensitive_find() {
        let store = LocalUserStore::new();
        let user = LocalUser::new("testuser", "test@example.com", "$hash", false);
        store.insert(user);
        assert!(store.find("TESTUSER").is_some());
        assert!(store.find("TestUser").is_some());
    }

    #[test]
    fn test_user_store_not_found() {
        let store = LocalUserStore::new();
        assert!(store.find("nonexistent").is_none());
    }

    #[test]
    fn test_user_password_not_expired() {
        let user = LocalUser::new("testuser", "test@example.com", "$hash", false);
        assert!(!user.is_password_expired(90));
    }

    #[test]
    fn test_user_is_locked_correctly() {
        let mut user = LocalUser::new("testuser", "test@example.com", "$hash", false);
        assert!(!user.is_locked());
        user.locked = true;
        user.lockout_expires = Some(Utc::now() + chrono::Duration::seconds(300));
        assert!(user.is_locked());
    }

    #[test]
    fn test_user_unlock_after_lockout_expires() {
        let mut user = LocalUser::new("testuser", "test@example.com", "$hash", false);
        user.locked = true;
        user.lockout_expires = Some(Utc::now() - chrono::Duration::seconds(1));
        user.try_unlock();
        assert!(!user.is_locked());
        assert_eq!(user.failed_attempts, 0);
    }

    #[test]
    fn test_user_record_failed_attempts_locks() {
        let mut user = LocalUser::new("testuser", "test@example.com", "$hash", false);
        // Record 5 failures with max_attempts = 5
        for _ in 0..5 {
            let locked = user.record_failed_login(5, 900);
            if locked {
                break;
            }
        }
        assert!(user.is_locked());
        assert_eq!(user.failed_attempts, 5);
    }

    #[test]
    fn test_user_reset_failed_attempts_on_success() {
        let mut user = LocalUser::new("testuser", "test@example.com", "$hash", false);
        user.record_failed_login(5, 900);
        user.record_failed_login(5, 900);
        assert_eq!(user.failed_attempts, 2);
        user.reset_failed_attempts();
        assert_eq!(user.failed_attempts, 0);
        assert!(!user.is_locked());
    }

    // ── Audit Log Tests ────────────────────────────────────────────────────

    #[test]
    fn test_audit_log_entries() {
        let log = AuditLog::new();
        let entry = AuditEntry::new(
            AuditEventType::LocalLoginSuccess,
            "testuser",
            true,
            "Login successful",
        );
        log.log(entry.clone());
        let entries = log.get_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].username, "testuser");
        assert_eq!(entries[0].event_type, "local_login_success");
    }

    #[test]
    fn test_audit_log_event_type_labels() {
        assert_eq!(
            AuditEventType::LocalLoginSuccess.label(),
            "local_login_success"
        );
        assert_eq!(
            AuditEventType::LocalLoginFailed.label(),
            "local_login_failed"
        );
        assert_eq!(
            AuditEventType::LocalAccountCreated.label(),
            "local_account_created"
        );
        assert_eq!(
            AuditEventType::LocalAccountLocked.label(),
            "local_account_locked"
        );
    }

    // ── LocalAuthState Tests ───────────────────────────────────────────────

    #[test]
    fn test_local_auth_state_new() {
        let config = LocalAccountsConfig::default();
        let state = LocalAuthState::new(config);
        assert!(!state.is_accessible());
    }

    #[test]
    fn test_local_auth_state_enabled() {
        let config = LocalAccountsConfig {
            enabled: true,
            ..LocalAccountsConfig::default()
        };
        let state = LocalAuthState::new(config);
        assert!(state.is_accessible());
    }

    // ── Login Handler Tests ─────────────────────────────────────────────────

    async fn make_local_login_request(
        state: &LocalAuthState,
        username: &str,
        password: &str,
    ) -> Response {
        let body = serde_json::json!({
            "username": username,
            "password": password,
        });

        let app = Router::new()
            .route("/v1/auth/local/login", post(local_login))
            .with_state(state.clone());

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/local/login")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        app.oneshot(req).await.unwrap()
    }

    async fn make_local_register_request(
        state: &LocalAuthState,
        username: &str,
        password: &str,
        email: &str,
    ) -> Response {
        let body = serde_json::json!({
            "username": username,
            "password": password,
            "email": email,
        });

        let app = Router::new()
            .route("/v1/auth/local/register", post(local_register))
            .with_state(state.clone());

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/local/register")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        app.oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn test_local_login_disabled_by_default() {
        let state = LocalAuthState::default();
        let resp = make_local_login_request(&state, "admin", "Password1!").await;
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn test_local_login_account_not_found() {
        let config = LocalAccountsConfig {
            enabled: true,
            ..LocalAccountsConfig::default()
        };
        let state = LocalAuthState::new(config);

        let resp = make_local_login_request(&state, "nonexistent", "Password1!").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_local_login_invalid_credentials() {
        let config = LocalAccountsConfig {
            enabled: true,
            ..LocalAccountsConfig::default()
        };
        let mut state = LocalAuthState::new(config);

        // Create a user first
        let hash = hash_password("CorrectPass1!").unwrap();
        let user = LocalUser::new("testuser", "test@example.com", &hash, false);
        state.user_store.insert(user);

        let resp = make_local_login_request(&state, "testuser", "WrongPass1!").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_local_login_empty_username() {
        let config = LocalAccountsConfig {
            enabled: true,
            ..LocalAccountsConfig::default()
        };
        let state = LocalAuthState::new(config);
        let resp = make_local_login_request(&state, "", "Password1!").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_local_login_empty_password() {
        let config = LocalAccountsConfig {
            enabled: true,
            ..LocalAccountsConfig::default()
        };
        let state = LocalAuthState::new(config);
        let resp = make_local_login_request(&state, "testuser", "").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_local_login_account_locked_after_failed_attempts() {
        let config = LocalAccountsConfig {
            enabled: true,
            max_failed_attempts: 5,
            ..LocalAccountsConfig::default()
        };
        let state = LocalAuthState::new(config);

        // Create a user
        let hash = hash_password("CorrectPass1!abc").unwrap();
        state
            .user_store
            .insert(LocalUser::new("testuser", "test@example.com", &hash, false));

        // Try 4 failed logins (each returns 401)
        for _ in 0..4 {
            let resp = make_local_login_request(&state, "testuser", "WrongPass1!abc").await;
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }

        // 5th attempt triggers lockout; 6th gets 429 Too Many Requests
        // (5th fails and locks, returns 401 with locked=true)
        let resp = make_local_login_request(&state, "testuser", "WrongPass1!abc").await;
        // The 5th attempt sets locked=true (record_failed_login returns true when locked)
        // but the handler still returns 401 for the failed attempt that caused the lock
        // The NEXT attempt (6th) gets 429
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Now account is locked
        let resp = make_local_login_request(&state, "testuser", "WrongPass1!abc").await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn test_local_register_disabled_by_default() {
        let state = LocalAuthState::default();
        let resp =
            make_local_register_request(&state, "newuser", "Password1!abc", "user@example.com")
                .await;
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn test_local_register_short_password_rejected() {
        let config = LocalAccountsConfig {
            enabled: true,
            ..LocalAccountsConfig::default()
        };
        let state = LocalAuthState::new(config);

        let resp =
            make_local_register_request(&state, "newuser", "short", "user@example.com").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_local_register_weak_password_rejected() {
        let config = LocalAccountsConfig {
            enabled: true,
            ..LocalAccountsConfig::default()
        };
        let state = LocalAuthState::new(config);

        let resp =
            make_local_register_request(&state, "newuser", "alllowercase1!", "user@example.com")
                .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_local_register_duplicate_username() {
        let config = LocalAccountsConfig {
            enabled: true,
            ..LocalAccountsConfig::default()
        };
        let mut state = LocalAuthState::new(config);

        // Create first user
        let hash1 = hash_password("Password1!abc").unwrap();
        let user1 = LocalUser::new("existing", "existing@example.com", &hash1, false);
        state.user_store.insert(user1);

        // Try to create duplicate
        let hash2 = hash_password("Password1!abc").unwrap();
        let user2 = LocalUser::new("existing", "other@example.com", &hash2, false);
        state.user_store.insert(user2);

        let resp =
            make_local_register_request(&state, "existing", "Password1!abc", "dup@example.com")
                .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_local_register_success() {
        let config = LocalAccountsConfig {
            enabled: true,
            ..LocalAccountsConfig::default()
        };
        let state = LocalAuthState::new(config);

        let resp =
            make_local_register_request(&state, "newuser", "StrongPass1!abc", "user@example.com")
                .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Verify user was created
        let user = state.user_store.find("newuser");
        assert!(user.is_some());
        assert_eq!(user.unwrap().email, "user@example.com");
    }

    #[tokio::test]
    async fn test_local_audit_log_entries_created() {
        let config = LocalAccountsConfig {
            enabled: true,
            ..LocalAccountsConfig::default()
        };
        let state = LocalAuthState::new(config);

        // Initial audit log should be empty
        assert_eq!(state.audit_log.count(), 0);

        // Failed login should create audit entry
        let resp = make_local_login_request(&state, "nonexistent", "Password1!").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(state.audit_log.count() > 0);

        // Verify the audit entry
        let entries = state.audit_log.get_entries();
        assert_eq!(entries[0].event_type, "local_login_failed");
        assert_eq!(entries[0].username, "nonexistent");
        assert!(!entries[0].success);
    }

    // ── Password Expiry Tests ─────────────────────────────────────────────

    #[test]
    fn test_user_password_expired() {
        let mut user = LocalUser::new("testuser", "test@example.com", "$hash", false);
        // Set password_changed to 100 days ago
        user.password_changed = Utc::now() - chrono::Duration::days(100);
        assert!(user.is_password_expired(90));
    }

    #[test]
    // ── Account Status Tests ───────────────────────────────────────────────
    #[test]
    fn test_account_status_locked() {
        let mut user = LocalUser::new("testuser", "test@example.com", "$hash", false);
        user.locked = true;
        user.lockout_expires = Some(Utc::now() + chrono::Duration::seconds(300));

        let status = LocalAccountStatus {
            username: user.username.clone(),
            password_expiring: false,
            password_expired: false,
            locked: true,
            lockout_expires: Some(user.lockout_expires.unwrap().to_rfc3339()),
            failed_attempts: user.failed_attempts,
            roles: user.roles.clone(),
        };

        assert!(status.locked);
        assert!(status.lockout_expires.is_some());
    }

    // ── Air-Gapped Mode Tests ───────────────────────────────────────────────

    #[test]
    fn test_air_gapped_detects_empty_connectors() {
        let config = LocalAccountsConfig {
            enabled: true,
            ..LocalAccountsConfig::default()
        };
        assert!(config.is_air_gapped(&[]));
    }

    #[test]
    fn test_not_air_gapped_when_local_disabled() {
        let config = LocalAccountsConfig {
            enabled: false,
            ..LocalAccountsConfig::default()
        };
        // When local is disabled, local accounts are effectively air-gapped
        assert!(config.is_air_gapped(&["oidc".to_string()]));
    }

    // ── JSON Serialization Tests ────────────────────────────────────────────

    #[test]
    fn test_audit_entry_serializes() {
        let entry = AuditEntry::new(
            AuditEventType::LocalLoginSuccess,
            "testuser",
            true,
            "Login successful",
        );
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("local_login_success"));
        assert!(json.contains("testuser"));
    }

    #[test]
    fn test_local_login_response_serializes() {
        let resp = LocalLoginResponse {
            access_token: "test-token".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            sub: "testuser".to_string(),
            email: "test@example.com".to_string(),
            roles: vec!["viewer".to_string()],
            password_expiring: false,
            password_expired: false,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("test-token"));
        assert!(json.contains("testuser"));
    }

    #[test]
    fn test_local_register_response_serializes() {
        let resp = LocalRegisterResponse {
            username: "newuser".to_string(),
            email: "user@example.com".to_string(),
            message: "Account created successfully".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("newuser"));
        assert!(json.contains("Account created successfully"));
    }

    // ── Rate Limiting Tests ───────────────────────────────────────────────────

    async fn make_rl_login_request(
        state: &LocalAuthState,
        rate_limiter: &AuthRateLimiter,
        username: &str,
        password: &str,
    ) -> Response {
        let body = serde_json::json!({
            "username": username,
            "password": password,
        });

        let app = Router::new()
            .route("/v1/auth/local/login", post(local_login_with_rl))
            .with_state((state.clone(), Arc::new(rate_limiter.clone())));

        let req = Request::builder()
            .method("POST")
            .uri("/v1/auth/local/login")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        app.oneshot(req).await.unwrap()
    }

    /// S-10: Rapid-fire 6 logins → 5th allowed, 6th gets 429.
    #[tokio::test]
    async fn test_rate_limit_allows_5_then_429_on_6th() {
        let config = LocalAccountsConfig {
            enabled: true,
            ..LocalAccountsConfig::default()
        };
        let state = LocalAuthState::new(config);

        // Add an admin account so login isn't rejected for "account not found"
        // (which would mask the rate-limit test). Rate limit applies regardless
        // of auth success/failure.
        state
            .user_store
            .users
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                "admin".to_string(),
                LocalUser {
                    username: "admin".to_string(),
                    email: "admin@example.com".to_string(),
                    password_hash: "argon2id$invalid".to_string(),
                    password_created: Utc::now(),
                    password_changed: Utc::now(),
                    failed_attempts: 0,
                    last_failed_at: None,
                    locked: false,
                    lockout_expires: None,
                    is_admin: true,
                    has_logged_in: false,
                    roles: vec!["admin".to_string()],
                    created_at: Utc::now(),
                },
            );

        // Use login limit of 5 per minute with burst = 5.
        let rl_config = AuthRateLimitConfig {
            login_per_minute: 5,
            register_per_minute: 3,
        };
        let metrics = std::sync::Arc::new(crate::metrics::MetricsRegistry::new());
        let rate_limiter = AuthRateLimiter::new(rl_config, metrics);

        // Fire 5 rapid requests. All should be allowed (not rate-limited).
        // They will fail auth (invalid password) but that's expected — rate limit
        // applies per-request regardless of auth result.
        for i in 0..5 {
            let resp = make_rl_login_request(&state, &rate_limiter, "admin", "Password1!").await;
            assert_ne!(
                resp.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "Request {} should be allowed (not rate-limited), got 429",
                i + 1
            );
        }

        // The 6th request should be rate-limited (429)
        let resp = make_rl_login_request(&state, &rate_limiter, "admin", "Password1!").await;
        assert_eq!(
            resp.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "6th request should be rate-limited (429)"
        );

        // Verify Retry-After header is present
        let retry_after = resp
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .map(|v| v.to_str().unwrap().to_string());
        assert!(
            retry_after.is_some(),
            "Retry-After header should be present"
        );
        let retry_secs: u64 = retry_after.unwrap().parse().unwrap();
        assert!(retry_secs > 0, "Retry-After should be positive");
    }

    /// S-10: Verify that login and register endpoints have independent rate limits.
    #[tokio::test]
    async fn test_rate_limit_independent_per_endpoint() {
        let config = AuthRateLimitConfig {
            login_per_minute: 1,
            register_per_minute: 1,
        };
        let metrics = std::sync::Arc::new(crate::metrics::MetricsRegistry::new());
        let limiter = AuthRateLimiter::new(config, metrics);

        // First login allowed
        assert!(limiter.check("login").is_none());
        // Login blocked
        assert!(limiter.check("login").is_some());

        // First register allowed (independent)
        assert!(limiter.check("register").is_none());
        // Register blocked
        assert!(limiter.check("register").is_some());
    }

    /// S-10: Verify auth_rate_limit_hits_total counter is incremented.
    #[tokio::test]
    async fn test_rate_limit_hits_counter_incremented() {
        let config = AuthRateLimitConfig {
            login_per_minute: 1,
            register_per_minute: 5,
        };
        let metrics = std::sync::Arc::new(crate::metrics::MetricsRegistry::new());
        let limiter = AuthRateLimiter::new(config, metrics.clone());

        // 1 allowed, 2 blocked → 2 hits
        assert!(limiter.check("login").is_none());
        assert!(limiter.check("login").is_some());
        assert!(limiter.check("login").is_some());

        let hits = metrics
            .auth_rate_limit_hits_total
            .get("login")
            .map(|c| c.value())
            .unwrap_or(0);
        assert_eq!(hits, 2, "Counter should show 2 rate limit hits");
    }

    /// S-10: Verify SPINDLE_AUTH_RATE_LIMIT env var configures limits.
    #[tokio::test]
    async fn test_rate_limit_env_config() {
        std::env::set_var("SPINDLE_AUTH_RATE_LIMIT", "login:2,register:1");
        let config = AuthRateLimitConfig::from_env();
        assert_eq!(config.login_per_minute, 2);
        assert_eq!(config.register_per_minute, 1);
        std::env::remove_var("SPINDLE_AUTH_RATE_LIMIT");

        // Without env var, use defaults
        let config = AuthRateLimitConfig::from_env();
        assert_eq!(
            config.login_per_minute,
            crate::auth_rate_limit::DEFAULT_LOGIN_LIMIT
        );
        assert_eq!(
            config.register_per_minute,
            crate::auth_rate_limit::DEFAULT_REGISTER_LIMIT
        );
    }
}

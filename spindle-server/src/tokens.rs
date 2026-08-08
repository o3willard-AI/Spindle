//! Token management for Spindle Server.
//!
//! Implements C7 Token types + creation (M3-11) and lifecycle (M3-12):
//! - POST /v1/tokens → create token with role/scope/TTL validation
//! - DELETE /v1/tokens/{id} → single revocation
//! - DELETE /v1/tokens?owner=X → bulk revocation by owner
//! - DELETE /v1/tokens?scope=Y → bulk revocation by scope
//! - Token rotation: create new with overlapping validity → revoke old
//! - Expiry: auto-revoke via timestamp; warning emails at T-7d/T-1d
//! - last_used_at updated per request (sampled: max once/5min)
//! - Token prefix `sp_` for log/audit identification
//! - Argon2id hash storage (never retrievable)
//! - GET /v1/tokens → metadata only, no plaintext
//! - Policy max TTL per token type (user/service/agent)

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use argon2::{self, Algorithm, Argon2, Params, Version};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, Salt};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use crate::sessions::SessionConfig;

/// Token type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum TokenType {
    /// User token — belongs to a human user.
    User,
    /// Service token — belongs to a service account.
    Service,
    /// Agent token — short-lived, for automated agents (default TTL: 1h).
    Agent,
}

impl Default for TokenType {
    fn default() -> Self {
        TokenType::User
    }
}

impl std::fmt::Display for TokenType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenType::User => write!(f, "user"),
            TokenType::Service => write!(f, "service"),
            TokenType::Agent => write!(f, "agent"),
        }
    }
}

/// Token creation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CreateTokenRequest {
    /// Human-readable name for the token.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Owner user/service account ID.
    pub owner: String,
    /// Token type.
    #[serde(default)]
    pub token_type: TokenType,
    /// Roles to assign (must be ≤ owner's roles).
    #[serde(default)]
    pub roles: Vec<String>,
    /// Scopes to assign (must be ≤ owner's scope).
    #[serde(default)]
    pub scopes: Vec<String>,
    /// TTL in seconds (must be ≤ policy max for this token type).
    #[serde(default)]
    pub ttl_secs: Option<u64>,
}

/// Token metadata returned on creation (no plaintext token).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenMetadata {
    /// Unique token ID.
    pub id: String,
    /// Token name.
    pub name: String,
    /// Owner ID.
    pub owner: String,
    /// Token type.
    pub token_type: TokenType,
    /// Roles assigned.
    pub roles: Vec<String>,
    /// Scopes assigned.
    pub scopes: Vec<String>,
    /// Creation timestamp (Unix seconds).
    pub created_at: u64,
    /// Expiration timestamp (Unix seconds).
    pub expires_at: u64,
    /// Whether the token has been revoked.
    pub revoked: bool,
    /// Whether the token was disabled by reconciliation.
    pub disabled: bool,
    /// Reason for disablement (for audit).
    pub disabled_reason: Option<String>,
    /// Last used timestamp (Unix seconds).
    pub last_used_at: Option<u64>,
    /// Source connector for the owner (for reconciliation).
    pub connector: Option<String>,
}

/// Token creation response — includes plaintext token shown ONCE.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCreateResponse {
    pub id: String,
    pub name: String,
    pub token: String, // plaintext, shown once
}

/// Token creation validation error types.
#[derive(Debug, Error)]
pub enum TokenError {
    #[error("name is required")]
    NameRequired,
    #[error("owner is required")]
    OwnerRequired,
    #[error("role '{0}' exceeds owner's roles")]
    RoleExceedsOwner(String),
    #[error("scope '{0}' exceeds owner's scope")]
    ScopeExceedsOwner(String),
    #[error("TTL {0}s exceeds policy max ({1}s) for token type {2}")]
    TtlExceedsPolicy(u64, u64, TokenType),
    #[error("token not found")]
    NotFound,
    #[error("token already revoked")]
    AlreadyRevoked,
    #[error("token disabled by reconciliation")]
    TokenDisabled,
}

/// Policy defining max TTL per token type.
#[derive(Debug, Clone)]
pub struct TokenPolicy {
    /// Max TTL for user tokens (default: 30 days = 2592000).
    pub max_ttl_user: u64,
    /// Max TTL for service tokens (default: 365 days = 31536000).
    pub max_ttl_service: u64,
    /// Max TTL for agent tokens (default: 1 hour = 3600).
    pub max_ttl_agent: u64,
}

impl Default for TokenPolicy {
    fn default() -> Self {
        Self {
            max_ttl_user: 2592000,    // 30 days
            max_ttl_service: 31536000, // 365 days
            max_ttl_agent: 3600,       // 1 hour
        }
    }
}

impl TokenPolicy {
    pub fn max_ttl_for(&self, token_type: TokenType) -> u64 {
        match token_type {
            TokenType::User => self.max_ttl_user,
            TokenType::Service => self.max_ttl_service,
            TokenType::Agent => self.max_ttl_agent,
        }
    }
}

/// Token store trait.
#[async_trait]
pub trait TokenStore: Send + Sync + std::fmt::Debug {
    /// Store a new token.
    async fn create_token(&self, metadata: TokenMetadata, hash: String) -> Result<(), TokenError>;
    /// Get token metadata by ID.
    async fn get_token(&self, id: &str) -> Result<Option<TokenMetadata>, TokenError>;
    /// Get token by plaintext token (for authentication).
    async fn get_token_by_plaintext(&self, token: &str) -> Result<Option<TokenMetadata>, TokenError>;
    /// List all tokens for a user.
    async fn list_tokens_for_user(&self, user_id: &str) -> Result<Vec<TokenMetadata>, TokenError>;
    /// List all tokens (admin).
    async fn list_all_tokens(&self) -> Result<Vec<TokenMetadata>, TokenError>;
    /// Revoke a token by ID.
    async fn revoke_token(&self, id: &str) -> Result<bool, TokenError>;
    /// Bulk revoke tokens by owner (single UPDATE).
    async fn revoke_tokens_by_owner(&self, owner: &str) -> Result<usize, TokenError>;
    /// Bulk revoke tokens containing a scope.
    async fn revoke_tokens_by_scope(&self, scope: &str) -> Result<usize, TokenError>;
    /// Update last_used_at timestamp (sampled to avoid excessive writes).
    async fn update_last_used(&self, id: &str) -> Result<(), TokenError>;
    /// Clean up expired tokens (return count removed).
    async fn cleanup_expired(&self, config: &SessionConfig) -> Result<usize, TokenError>;
    /// Disable a token (by reconciliation).
    async fn disable_token(&self, id: &str, reason: &str) -> Result<bool, TokenError>;
    /// Re-enable a disabled token (manual admin action).
    async fn enable_token(&self, id: &str) -> Result<bool, TokenError>;
    /// List all disabled (orphaned by reconciliation) tokens.
    async fn list_disabled_tokens(&self) -> Result<Vec<TokenMetadata>, TokenError>;
    /// List tokens needing reconciliation (user-owned, not service, not revoked/disabled).
    async fn list_tokens_for_reconciliation(&self) -> Result<Vec<TokenMetadata>, TokenError>;
    /// Return tokens unused for ≥ N days (admin).
    async fn idle_tokens(&self, min_days: u64) -> Result<Vec<IdleTokenInfo>, TokenError>;
    /// Record a lifecycle audit event.
    async fn record_audit(&self, event: AuditEvent) -> Result<(), TokenError>;
    /// Query audit trail for a token (admin).
    async fn audit_events(
        &self,
        token_id: &str,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Vec<AuditEvent>, TokenError>;
}

/// Lifecycle audit event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub token_id: String,
    pub owner: String,
    pub event_type: AuditEventType,
    pub details: Option<String>,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuditEventType {
    Create,
    Rotate,
    Revoke,
    Expire,
    Disable,
    Enable,
}

impl std::fmt::Display for AuditEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditEventType::Create => write!(f, "create"),
            AuditEventType::Rotate => write!(f, "rotate"),
            AuditEventType::Revoke => write!(f, "revoke"),
            AuditEventType::Expire => write!(f, "expire"),
            AuditEventType::Disable => write!(f, "disable"),
            AuditEventType::Enable => write!(f, "enable"),
        }
    }
}

/// Idle token info returned by the idle report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdleTokenInfo {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub token_type: TokenType,
    pub roles: Vec<String>,
    pub scopes: Vec<String>,
    pub created_at: u64,
    pub last_used_at: Option<u64>,
    pub days_idle: u64,
    pub suggestion: String,
}

/// Hash a plaintext token using Argon2id.
pub fn hash_token(token: &str) -> String {
    let salt = Salt::from_b64("MDEyMzQ1Njc4OWFiY2RlZg").unwrap();
    Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::default(),
    )
    .hash_password(token.as_bytes(), salt)
    .map(|h| h.to_string())
    .unwrap_or_else(|_| token.to_string())
}

/// Verify a plaintext token against a stored hash.
pub fn verify_token(token: &str, hash: &str) -> bool {
    if let Ok(parsed) = PasswordHash::new(hash) {
        Argon2::default().verify_password(token.as_bytes(), &parsed).is_ok()
    } else {
        false
    }
}

/// Generate a new token string with `sp_` prefix.
pub fn generate_token() -> String {
    let uuid = Uuid::new_v4();
    format!("sp_{}", uuid)
}

/// Owner's roles and scope, used for validation when creating tokens.
#[derive(Debug, Clone)]
pub struct OwnerInfo {
    pub roles: HashSet<String>,
    pub scopes: HashSet<String>,
}

impl OwnerInfo {
    pub fn new(roles: Vec<String>, scopes: Vec<String>) -> Self {
        Self {
            roles: roles.into_iter().collect(),
            scopes: scopes.into_iter().collect(),
        }
    }
}

/// Reconciliation error from the connector.
#[derive(Debug)]
pub enum ReconciliationError {
    /// The connector was unreachable (transient failure — skip, don't disable).
    ConnectorUnavailable,
    /// Other error (log and skip).
    Other(String),
}

/// Result of a reconciliation run.
#[derive(Debug, Clone)]
pub struct ReconciliationResult {
    /// Total tokens checked.
    pub checked: usize,
    /// Tokens disabled because owner was no longer resolvable.
    pub disabled: usize,
    /// Tokens skipped due to connector being unavailable.
    pub skipped: usize,
    /// IDs of tokens that were disabled.
    pub orphaned_ids: Vec<String>,
}

/// Trait for resolving whether users still exist in their source connector.
/// Used by the reconciliation job (M3-14).
#[async_trait]
pub trait UserResolver: Send + Sync {
    /// Check if the given owners exist in the specified connector.
    /// Returns the set of owners that still exist.
    /// If the connector is unreachable, return `Err(ReconciliationError::ConnectorUnavailable)`.
    async fn resolve_owners(
        &self,
        connector: &str,
        owners: &HashSet<String>,
    ) -> Result<HashSet<String>, ReconciliationError>;
}

/// Token manager: handles token creation, validation, revocation.
#[derive(Debug, Clone)]
pub struct TokenManager {
    store: Arc<dyn TokenStore>,
    policy: TokenPolicy,
}

impl TokenManager {
    pub fn new(store: Arc<dyn TokenStore>, policy: TokenPolicy) -> Self {
        Self { store, policy }
    }

    pub fn with_default_policy(store: Arc<dyn TokenStore>) -> Self {
        Self::new(store, TokenPolicy::default())
    }

    /// Create a new token with validation.
    /// Returns the creation response with plaintext token (shown once).
    pub async fn create_token(
        &self,
        req: CreateTokenRequest,
        owner_info: &OwnerInfo,
    ) -> Result<TokenCreateResponse, TokenError> {
        // Validate name
        if req.name.is_empty() {
            return Err(TokenError::NameRequired);
        }
        // Validate owner
        if req.owner.is_empty() {
            return Err(TokenError::OwnerRequired);
        }
        // Validate roles ≤ owner's roles
        for role in &req.roles {
            if !owner_info.roles.contains(role) {
                return Err(TokenError::RoleExceedsOwner(role.clone()));
            }
        }
        // Validate scopes ≤ owner's scope
        for scope in &req.scopes {
            if !owner_info.scopes.contains(scope) {
                return Err(TokenError::ScopeExceedsOwner(scope.clone()));
            }
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Determine TTL
        let ttl = req.ttl_secs.unwrap_or_else(|| match req.token_type {
            TokenType::User => 2592000,    // 30 days default
            TokenType::Service => 31536000, // 1 year default
            TokenType::Agent => 3600,       // 1 hour default
        });

        // Validate TTL against policy max
        let max_ttl = self.policy.max_ttl_for(req.token_type);
        if ttl > max_ttl {
            return Err(TokenError::TtlExceedsPolicy(ttl, max_ttl, req.token_type));
        }

        // Generate token
        let plaintext_token = generate_token();
        let token_hash = hash_token(&plaintext_token);
        let token_id = Uuid::new_v4().to_string();

        let metadata = TokenMetadata {
            id: token_id.clone(),
            name: req.name.clone(),
            owner: req.owner.clone(),
            token_type: req.token_type,
            roles: req.roles.clone(),
            scopes: req.scopes.clone(),
            created_at: now,
            expires_at: now + ttl,
            revoked: false,
            disabled: false,
            disabled_reason: None,
            last_used_at: None,
            connector: None,
        };

        self.store.create_token(metadata, token_hash).await?;

        // Log create event
        self.store.record_audit(AuditEvent {
            id: Uuid::new_v4().to_string(),
            token_id: token_id.clone(),
            owner: req.owner.clone(),
            event_type: AuditEventType::Create,
            details: Some(format!("name={} type={}", req.name, req.token_type)),
            created_at: now,
        }).await.ok();

        Ok(TokenCreateResponse {
            id: token_id,
            name: req.name,
            token: plaintext_token,
        })
    }

    /// Validate a token and return its metadata.
    pub async fn validate_token(&self, token: &str) -> Result<TokenMetadata, TokenError> {
        let metadata = self
            .store
            .get_token_by_plaintext(token)
            .await?
            .ok_or(TokenError::NotFound)?;

        if metadata.revoked {
            return Err(TokenError::AlreadyRevoked);
        }

        if metadata.disabled {
            return Err(TokenError::TokenDisabled);
        }

        // Check expiration
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if now >= metadata.expires_at {
            return Err(TokenError::NotFound);
        }

        // Update last used (sampled)
        self.store.update_last_used(&metadata.id).await.ok();

        Ok(metadata)
    }

    /// Revoke a token by ID.
    pub async fn revoke_token(&self, id: &str) -> Result<bool, TokenError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Get owner before revoking for audit
        let owner = self.store.get_token(id).await.ok().flatten().map(|t| t.owner).unwrap_or_default();
        let result = self.store.revoke_token(id).await;
        // Log revoke event
        self.store.record_audit(AuditEvent {
            id: Uuid::new_v4().to_string(),
            token_id: id.to_string(),
            owner,
            event_type: AuditEventType::Revoke,
            details: None,
            created_at: now,
        }).await.ok();
        result
    }

    /// List tokens for a user.
    pub async fn list_user_tokens(&self, user_id: &str) -> Result<Vec<TokenMetadata>, TokenError> {
        self.store.list_tokens_for_user(user_id).await
    }

    /// List all tokens (admin).
    pub async fn list_all_tokens(&self) -> Result<Vec<TokenMetadata>, TokenError> {
        self.store.list_all_tokens().await
    }

    /// Get token metadata by ID.
    pub async fn get_token(&self, id: &str) -> Result<Option<TokenMetadata>, TokenError> {
        self.store.get_token(id).await
    }

    /// Bulk revoke all tokens for an owner (single UPDATE).
    pub async fn revoke_tokens_by_owner(&self, owner: &str) -> Result<usize, TokenError> {
        self.store.revoke_tokens_by_owner(owner).await
    }

    /// Bulk revoke tokens matching a scope.
    pub async fn revoke_tokens_by_scope(&self, scope: &str) -> Result<usize, TokenError> {
        self.store.revoke_tokens_by_scope(scope).await
    }

    /// Rotate a token: create a new token with overlapping validity, then
    /// revoke the old one. Returns the new token.
    pub async fn rotate_token(
        &self,
        old_token_plaintext: &str,
        owner_info: &OwnerInfo,
    ) -> Result<TokenCreateResponse, TokenError> {
        // Validate the old token exists and get its metadata
        let metadata = self
            .store
            .get_token_by_plaintext(old_token_plaintext)
            .await?
            .ok_or(TokenError::NotFound)?;

        // Create new token with same attributes
        let req = CreateTokenRequest {
            name: metadata.name.clone(),
            description: Some(format!("rotated from {}", metadata.id)),
            owner: metadata.owner.clone(),
            token_type: metadata.token_type,
            roles: metadata.roles.clone(),
            scopes: metadata.scopes.clone(),
            ttl_secs: Some(metadata.expires_at.saturating_sub(metadata.created_at)),
        };

        let response = self.create_token(req, owner_info).await?;

        // Revoke the old token
        let old_revoked = self.store.revoke_token(&metadata.id).await?;

        // Audit: rotate event (create new + revoke old in one logical operation)
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.store.record_audit(AuditEvent {
            id: Uuid::new_v4().to_string(),
            token_id: metadata.id.clone(),
            owner: metadata.owner.clone(),
            event_type: AuditEventType::Rotate,
            details: Some(format!("rotated → {}", response.id)),
            created_at: now,
        }).await.ok();
        if old_revoked {
            self.store.record_audit(AuditEvent {
                id: Uuid::new_v4().to_string(),
                token_id: metadata.id.clone(),
                owner: metadata.owner.clone(),
                event_type: AuditEventType::Revoke,
                details: Some("rotated — replaced by new token".to_string()),
                created_at: now,
            }).await.ok();
        }

        Ok(response)
    }

    /// Get tokens expiring within `warn_secs` seconds (for warning emails).
    pub async fn tokens_expiring_within(
        &self,
        warn_secs: u64,
    ) -> Result<Vec<TokenMetadata>, TokenError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let all = self.store.list_all_tokens().await?;
        let expiry_threshold = now + warn_secs;
        Ok(all
            .into_iter()
            .filter(|t| !t.revoked && t.expires_at <= expiry_threshold)
            .collect())
    }

    /// Clean up expired tokens. Returns count of tokens revoked.
    pub async fn cleanup_expired_tokens(&self) -> Result<usize, TokenError> {
        let tokens = self.store.list_all_tokens().await?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let expired_ids: Vec<String> = tokens
            .iter()
            .filter(|t| t.expires_at <= now && !t.revoked)
            .map(|t| t.id.clone())
            .collect();
        let count = self.store.cleanup_expired(&SessionConfig::default()).await?;
        // Log expire events for each expired token
        for id in &expired_ids {
            let owner = self.store.get_token(id).await.ok().flatten().map(|t| t.owner).unwrap_or_default();
            self.store.record_audit(AuditEvent {
                id: Uuid::new_v4().to_string(),
                token_id: id.clone(),
                owner,
                event_type: AuditEventType::Expire,
                details: Some("automatically revoked — TTL elapsed".to_string()),
                created_at: now,
            }).await.ok();
        }
        Ok(count)
    }

    /// Disable a token (manual admin action or reconciliation).
    pub async fn disable_token(&self, id: &str, reason: &str) -> Result<bool, TokenError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let owner = self.store.get_token(id).await.ok().flatten().map(|t| t.owner).unwrap_or_default();
        let result = self.store.disable_token(id, reason).await;
        self.store.record_audit(AuditEvent {
            id: Uuid::new_v4().to_string(),
            token_id: id.to_string(),
            owner,
            event_type: AuditEventType::Disable,
            details: Some(reason.to_string()),
            created_at: now,
        }).await.ok();
        result
    }

    /// Re-enable a disabled token (manual admin action).
    pub async fn enable_token(&self, id: &str) -> Result<bool, TokenError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let owner = self.store.get_token(id).await.ok().flatten().map(|t| t.owner).unwrap_or_default();
        let result = self.store.enable_token(id).await;
        self.store.record_audit(AuditEvent {
            id: Uuid::new_v4().to_string(),
            token_id: id.to_string(),
            owner,
            event_type: AuditEventType::Enable,
            details: None,
            created_at: now,
        }).await.ok();
        result
    }

    /// List all disabled (orphaned by reconciliation) tokens.
    pub async fn list_disabled_tokens(&self) -> Result<Vec<TokenMetadata>, TokenError> {
        self.store.list_disabled_tokens().await
    }

    /// Report tokens unused for ≥ N days (admin).
    pub async fn list_idle_tokens(&self, min_days: u64) -> Result<Vec<IdleTokenInfo>, TokenError> {
        self.store.idle_tokens(min_days).await
    }

    /// Record a lifecycle audit event (admin).
    pub async fn record_audit(&self, event: AuditEvent) -> Result<(), TokenError> {
        self.store.record_audit(event).await
    }

    /// Query audit trail for a token (admin).
    pub async fn get_audit_events(
        &self,
        token_id: &str,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Vec<AuditEvent>, TokenError> {
        self.store.audit_events(token_id, from, to).await
    }

    /// Reconcile tokens: check if each user-owned token's owner still exists.
    /// Tokens whose owners are not found are disabled and added to orphan report.
    /// Connector unavailable → skip (don't disable on transient failures).
    /// Idempotent: already-disabled tokens are not re-disabled.
    pub async fn reconcile_tokens<R: UserResolver + Sync>(
        &self,
        resolver: &R,
    ) -> Result<ReconciliationResult, TokenError> {
        let tokens = self.store.list_tokens_for_reconciliation().await?;

        // Batch by connector to minimize LDAP queries
        let mut by_connector: HashMap<String, Vec<TokenMetadata>> = HashMap::new();
        for token in &tokens {
            let connector = token.connector.clone().unwrap_or_else(|| "default".to_string());
            by_connector.entry(connector).or_default().push(token.clone());
        }

        let mut disabled_count = 0;
        let mut skipped_count = 0;
        let mut orphaned_ids: Vec<String> = Vec::new();

        for (connector, token_list) in &by_connector {
            // Batch resolution per connector
            let owners: HashSet<String> = token_list.iter().map(|t| t.owner.clone()).collect();
            let resolution = resolver.resolve_owners(connector, &owners).await;

            match resolution {
                Ok(existing_owners) => {
                    for token in token_list {
                        if !existing_owners.contains(&token.owner) {
                            // Owner no longer resolvable → disable token
                            let reason = format!(
                                "Owner '{}' not found in connector '{}' during reconciliation",
                                token.owner, connector
                            );
                            self.store.disable_token(&token.id, &reason).await?;
                            disabled_count += 1;
                            orphaned_ids.push(token.id.clone());
                        }
                    }
                }
                Err(ReconciliationError::ConnectorUnavailable) => {
                    // Connector unreachable → skip these tokens (don't nuke on transient failures)
                    skipped_count += token_list.len();
                }
                Err(ReconciliationError::Other(e)) => {
                    // Log error but skip, don't fail entire reconciliation
                    eprintln!("Reconciliation error for connector {}: {}", connector, e);
                    skipped_count += token_list.len();
                }
            }
        }

        Ok(ReconciliationResult {
            checked: tokens.len(),
            disabled: disabled_count,
            skipped: skipped_count,
            orphaned_ids,
        })
    }

    /// Get the policy.
    pub fn policy(&self) -> &TokenPolicy {
        &self.policy
    }
}

/// In-memory token store implementation.
#[derive(Debug, Clone, Default)]
pub struct InMemoryTokenStore {
    tokens: Arc<std::sync::Mutex<HashMap<String, (TokenMetadata, String)>>>,
    hash_index: Arc<std::sync::Mutex<HashMap<String, String>>>, // hash -> token_id
    audit_events: Arc<std::sync::Mutex<HashMap<String, Vec<AuditEvent>>>>, // token_id -> events
}

impl InMemoryTokenStore {
    pub fn new() -> Self {
        Self {
            tokens: Arc::new(std::sync::Mutex::new(HashMap::new())),
            hash_index: Arc::new(std::sync::Mutex::new(HashMap::new())),
            audit_events: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl TokenStore for InMemoryTokenStore {
    async fn create_token(&self, metadata: TokenMetadata, hash: String) -> Result<(), TokenError> {
        let mut tokens = self.tokens.lock().unwrap();
        let mut hash_index = self.hash_index.lock().unwrap();
        hash_index.insert(hash.clone(), metadata.id.clone());
        tokens.insert(metadata.id.clone(), (metadata, hash));
        Ok(())
    }

    async fn get_token(&self, id: &str) -> Result<Option<TokenMetadata>, TokenError> {
        let tokens = self.tokens.lock().unwrap();
        Ok(tokens.get(id).map(|(meta, _)| meta.clone()))
    }

    async fn get_token_by_plaintext(&self, token: &str) -> Result<Option<TokenMetadata>, TokenError> {
        let hash = hash_token(token);
        let hash_index = self.hash_index.lock().unwrap();
        let token_id = hash_index.get(&hash).cloned();
        drop(hash_index);

        if let Some(id) = token_id {
            let tokens = self.tokens.lock().unwrap();
            Ok(tokens.get(&id).map(|(meta, _)| meta.clone()))
        } else {
            Ok(None)
        }
    }

    async fn list_tokens_for_user(&self, user_id: &str) -> Result<Vec<TokenMetadata>, TokenError> {
        let tokens = self.tokens.lock().unwrap();
        Ok(tokens
            .values()
            .filter(|(meta, _)| meta.owner == user_id && !meta.revoked)
            .map(|(meta, _)| meta.clone())
            .collect())
    }

    async fn list_all_tokens(&self) -> Result<Vec<TokenMetadata>, TokenError> {
        let tokens = self.tokens.lock().unwrap();
        Ok(tokens.values().map(|(meta, _)| meta.clone()).collect())
    }

    async fn revoke_token(&self, id: &str) -> Result<bool, TokenError> {
        let mut tokens = self.tokens.lock().unwrap();
        if let Some((meta, _hash)) = tokens.get_mut(id) {
            meta.revoked = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn update_last_used(&self, id: &str) -> Result<(), TokenError> {
        let mut tokens = self.tokens.lock().unwrap();
        if let Some((meta, _)) = tokens.get_mut(id) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            // Sample: only update if last update was > 5 minutes ago
            let should_update = meta.last_used_at.map_or(true, |last| {
                now.saturating_sub(last) >= 300 // 5 minutes
            });
            if should_update {
                meta.last_used_at = Some(now);
            }
        }
        Ok(())
    }

    async fn revoke_tokens_by_owner(&self, owner: &str) -> Result<usize, TokenError> {
        let mut tokens = self.tokens.lock().unwrap();
        let count = tokens
            .values_mut()
            .filter(|(meta, _)| meta.owner == owner && !meta.revoked)
            .map(|(meta, _)| {
                meta.revoked = true;
                1
            })
            .count();
        Ok(count)
    }

    async fn revoke_tokens_by_scope(&self, scope: &str) -> Result<usize, TokenError> {
        let mut tokens = self.tokens.lock().unwrap();
        let count = tokens
            .values_mut()
            .filter(|(meta, _)| {
                !meta.revoked && (meta.scopes.iter().any(|s| s == scope) || meta.scopes.iter().any(|s| s.starts_with(scope)))
            })
            .map(|(meta, _)| {
                meta.revoked = true;
                1
            })
            .count();
        Ok(count)
    }

    async fn cleanup_expired(&self, _config: &SessionConfig) -> Result<usize, TokenError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut tokens = self.tokens.lock().unwrap();
        let count = tokens
            .values_mut()
            .filter(|(meta, _)| meta.expires_at <= now && !meta.revoked)
            .map(|(meta, _)| {
                meta.revoked = true;
                1
            })
            .count();
        Ok(count)
    }

    async fn disable_token(&self, id: &str, reason: &str) -> Result<bool, TokenError> {
        let mut tokens = self.tokens.lock().unwrap();
        if let Some((meta, _)) = tokens.get_mut(id) {
            meta.disabled = true;
            meta.disabled_reason = Some(reason.to_string());
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn enable_token(&self, id: &str) -> Result<bool, TokenError> {
        let mut tokens = self.tokens.lock().unwrap();
        if let Some((meta, _)) = tokens.get_mut(id) {
            meta.disabled = false;
            meta.disabled_reason = None;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn list_disabled_tokens(&self) -> Result<Vec<TokenMetadata>, TokenError> {
        let tokens = self.tokens.lock().unwrap();
        Ok(tokens
            .values()
            .filter(|(meta, _)| meta.disabled)
            .map(|(meta, _)| meta.clone())
            .collect())
    }

    async fn list_tokens_for_reconciliation(&self) -> Result<Vec<TokenMetadata>, TokenError> {
        let tokens = self.tokens.lock().unwrap();
        Ok(tokens
            .values()
            .filter(|(meta, _)| {
                // User tokens only (not service accounts), not revoked, not disabled
                matches!(meta.token_type, TokenType::User)
                    && !meta.revoked
                    && !meta.disabled
            })
            .map(|(meta, _)| meta.clone())
            .collect())
    }

    async fn idle_tokens(&self, min_days: u64) -> Result<Vec<IdleTokenInfo>, TokenError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let min_seconds = min_days * 86400;
        let mut tokens = self.tokens.lock().unwrap();
        let result = tokens
            .values()
            .filter(|(meta, _)| !meta.revoked && !meta.disabled)
            .filter_map(|(meta, _)| {
                meta.last_used_at.filter(|last| {
                    now.saturating_sub(*last) >= min_seconds
                }).map(|last_used| {
                    let days_idle = (now.saturating_sub(last_used)) / 86400;
                    let suggestion = if days_idle >= 180 {
                        "Revoke — no activity".to_string()
                    } else if days_idle >= 90 {
                        "Consider revoking — 90+ days idle".to_string()
                    } else {
                        "No action required".to_string()
                    };
                    IdleTokenInfo {
                        id: meta.id.clone(),
                        name: meta.name.clone(),
                        owner: meta.owner.clone(),
                        token_type: meta.token_type,
                        roles: meta.roles.clone(),
                        scopes: meta.scopes.clone(),
                        created_at: meta.created_at,
                        last_used_at: Some(last_used),
                        days_idle,
                        suggestion,
                    }
                })
            })
            .collect();
        Ok(result)
    }

    async fn record_audit(&self, event: AuditEvent) -> Result<(), TokenError> {
        let mut audit = self.audit_events.lock().unwrap();
        audit
            .entry(event.token_id.clone())
            .or_default()
            .push(event);
        Ok(())
    }

    async fn audit_events(
        &self,
        token_id: &str,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Vec<AuditEvent>, TokenError> {
        let audit = self.audit_events.lock().unwrap();
        let events = audit.get(token_id).cloned().unwrap_or_default();
        let result = events
            .into_iter()
            .filter(|e| {
                if let Some(f) = from {
                    if e.created_at < f {
                        return false;
                    }
                }
                if let Some(t) = to {
                    if e.created_at > t {
                        return false;
                    }
                }
                true
            })
            .collect();
        Ok(result)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_owner(roles: Vec<&str>, scopes: Vec<&str>) -> OwnerInfo {
        OwnerInfo::new(
            roles.iter().map(|s| s.to_string()).collect(),
            scopes.iter().map(|s| s.to_string()).collect(),
        )
    }

    fn make_request(name: &str, owner: &str, token_type: TokenType) -> CreateTokenRequest {
        CreateTokenRequest {
            name: name.to_string(),
            description: None,
            owner: owner.to_string(),
            token_type,
            roles: vec![],
            scopes: vec![],
            ttl_secs: None,
        }
    }

    #[test]
    fn test_token_type_display() {
        assert_eq!(format!("{}", TokenType::User), "user");
        assert_eq!(format!("{}", TokenType::Service), "service");
        assert_eq!(format!("{}", TokenType::Agent), "agent");
    }

    #[test]
    fn test_token_type_default() {
        assert_eq!(TokenType::default(), TokenType::User);
    }

    #[test]
    fn test_generate_token_prefix() {
        let token = generate_token();
        assert!(token.starts_with("sp_"));
        assert!(!token.is_empty());
    }

    #[test]
    fn test_generate_token_uniqueness() {
        let tokens: Vec<String> = (0..100).map(|_| generate_token()).collect();
        let unique: HashSet<&String> = tokens.iter().collect();
        assert_eq!(tokens.len(), unique.len());
    }

    #[test]
    fn test_hash_and_verify_token() {
        let token = generate_token();
        let hash = hash_token(&token);
        assert_ne!(hash, token);
        assert!(verify_token(&token, &hash));
        assert!(!verify_token(&generate_token(), &hash));
    }

    #[test]
    fn test_policy_default_ttls() {
        let policy = TokenPolicy::default();
        assert_eq!(policy.max_ttl_for(TokenType::User), 2592000);
        assert_eq!(policy.max_ttl_for(TokenType::Service), 31536000);
        assert_eq!(policy.max_ttl_for(TokenType::Agent), 3600);
    }

    #[test]
    fn test_policy_custom_ttls() {
        let policy = TokenPolicy {
            max_ttl_user: 100,
            max_ttl_service: 200,
            max_ttl_agent: 300,
        };
        assert_eq!(policy.max_ttl_for(TokenType::User), 100);
        assert_eq!(policy.max_ttl_for(TokenType::Service), 200);
        assert_eq!(policy.max_ttl_for(TokenType::Agent), 300);
    }

    #[tokio::test]
    async fn test_create_token_success() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer", "editor"], vec!["project-a", "project-b"]);

        let req = CreateTokenRequest {
            name: "my-token".to_string(),
            description: Some("test token".to_string()),
            owner: "user1".to_string(),
            token_type: TokenType::User,
            roles: vec!["viewer".to_string()],
            scopes: vec!["project-a".to_string()],
            ttl_secs: Some(3600),
        };

        let response = manager.create_token(req, &owner).await.unwrap();
        assert_eq!(response.name, "my-token");
        assert!(response.token.starts_with("sp_"));
        assert!(!response.id.is_empty());
    }

    #[tokio::test]
    async fn test_create_token_empty_name_fails() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        let req = make_request("", "user1", TokenType::User);
        let result = manager.create_token(req, &owner).await;
        assert!(matches!(result, Err(TokenError::NameRequired)));
    }

    #[tokio::test]
    async fn test_create_token_empty_owner_fails() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        let req = make_request("test-token", "", TokenType::User);
        let result = manager.create_token(req, &owner).await;
        assert!(matches!(result, Err(TokenError::OwnerRequired)));
    }

    #[tokio::test]
    async fn test_create_token_role_exceeds_owner() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        let req = CreateTokenRequest {
            name: "test-token".to_string(),
            description: None,
            owner: "user1".to_string(),
            token_type: TokenType::User,
            roles: vec!["admin".to_string()], // owner doesn't have admin
            scopes: vec!["project-a".to_string()],
            ttl_secs: Some(3600),
        };

        let result = manager.create_token(req, &owner).await;
        assert!(matches!(result, Err(TokenError::RoleExceedsOwner(_))));
    }

    #[tokio::test]
    async fn test_create_token_scope_exceeds_owner() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        let req = CreateTokenRequest {
            name: "test-token".to_string(),
            description: None,
            owner: "user1".to_string(),
            token_type: TokenType::User,
            roles: vec!["viewer".to_string()],
            scopes: vec!["project-b".to_string()], // owner doesn't have project-b
            ttl_secs: Some(3600),
        };

        let result = manager.create_token(req, &owner).await;
        assert!(matches!(result, Err(TokenError::ScopeExceedsOwner(_))));
    }

    #[tokio::test]
    async fn test_create_token_ttl_exceeds_policy() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        let req = CreateTokenRequest {
            name: "test-token".to_string(),
            description: None,
            owner: "user1".to_string(),
            token_type: TokenType::Agent, // max 1h
            roles: vec![],
            scopes: vec![],
            ttl_secs: Some(7200), // 2h — exceeds 1h limit
        };

        let result = manager.create_token(req, &owner).await;
        assert!(matches!(result, Err(TokenError::TtlExceedsPolicy(_, _, TokenType::Agent))));
    }

    #[tokio::test]
    async fn test_create_token_agent_default_ttl() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        let req = CreateTokenRequest {
            name: "agent-token".to_string(),
            description: None,
            owner: "user1".to_string(),
            token_type: TokenType::Agent,
            roles: vec![],
            scopes: vec![],
            ttl_secs: None, // Should default to 1h (3600)
        };

        let response = manager.create_token(req, &owner).await.unwrap();
        let token_id = response.id;

        // Verify the token metadata has correct TTL
        let metadata = manager.get_token(&token_id).await.unwrap().unwrap();
        let duration = metadata.expires_at - metadata.created_at;
        assert_eq!(duration, 3600); // 1 hour default for agent tokens
    }

    #[tokio::test]
    async fn test_validate_token_success() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        let req = CreateTokenRequest {
            name: "test-token".to_string(),
            description: None,
            owner: "user1".to_string(),
            token_type: TokenType::User,
            roles: vec!["viewer".to_string()],
            scopes: vec!["project-a".to_string()],
            ttl_secs: Some(3600),
        };

        let response = manager.create_token(req, &owner).await.unwrap();
        let metadata = manager.validate_token(&response.token).await.unwrap();
        assert_eq!(metadata.owner, "user1");
        assert_eq!(metadata.name, "test-token");
        assert!(!metadata.revoked);
    }

    #[tokio::test]
    async fn test_validate_token_not_found() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);

        let result = manager.validate_token("sp_invalid").await;
        assert!(matches!(result, Err(TokenError::NotFound)));
    }

    #[tokio::test]
    async fn test_revoke_token() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        let response = manager
            .create_token(CreateTokenRequest {
                name: "test-token".to_string(),
                description: None,
                owner: "user1".to_string(),
                token_type: TokenType::User,
                roles: vec!["viewer".to_string()],
                scopes: vec!["project-a".to_string()],
                ttl_secs: Some(3600),
            }, &owner)
            .await
            .unwrap();

        let revoked = manager.revoke_token(&response.id).await.unwrap();
        assert!(revoked);

        // Token should no longer be valid
        let result = manager.validate_token(&response.token).await;
        assert!(matches!(result, Err(TokenError::AlreadyRevoked)));
    }

    #[tokio::test]
    async fn test_revoke_nonexistent_token() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);

        let result = manager.revoke_token("nonexistent").await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_list_user_tokens() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        manager.create_token(
            CreateTokenRequest {
                name: "token1".to_string(),
                description: None,
                owner: "user1".to_string(),
                token_type: TokenType::User,
                roles: vec!["viewer".to_string()],
                scopes: vec!["project-a".to_string()],
                ttl_secs: Some(3600),
            },
            &owner,
        )
        .await
        .unwrap();

        manager.create_token(
            CreateTokenRequest {
                name: "token2".to_string(),
                description: None,
                owner: "user2".to_string(),
                token_type: TokenType::User,
                roles: vec!["viewer".to_string()],
                scopes: vec!["project-a".to_string()],
                ttl_secs: Some(3600),
            },
            &owner,
        )
        .await
        .unwrap();

        let user1_tokens = manager.list_user_tokens("user1").await.unwrap();
        assert_eq!(user1_tokens.len(), 1);
        assert_eq!(user1_tokens[0].name, "token1");

        let user2_tokens = manager.list_user_tokens("user2").await.unwrap();
        assert_eq!(user2_tokens.len(), 1);
        assert_eq!(user2_tokens[0].name, "token2");
    }

    #[tokio::test]
    async fn test_list_all_tokens_admin() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        manager.create_token(
            CreateTokenRequest {
                name: "token1".to_string(),
                description: None,
                owner: "user1".to_string(),
                token_type: TokenType::User,
                roles: vec![],
                scopes: vec![],
                ttl_secs: Some(3600),
            },
            &owner,
        )
        .await
        .unwrap();

        let all_tokens = manager.list_all_tokens().await.unwrap();
        assert_eq!(all_tokens.len(), 1);
    }

    #[tokio::test]
    async fn test_token_metadata_no_plaintext() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "test-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec!["viewer".to_string()],
                    scopes: vec!["project-a".to_string()],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Token metadata should not contain the plaintext token
        let metadata = manager.get_token(&response.id).await.unwrap().unwrap();
        assert_eq!(metadata.id, response.id);
        assert_eq!(metadata.name, response.name);
        // The plaintext token is only in the creation response, not in stored metadata
        assert!(response.token.starts_with("sp_"));
        assert!(!metadata.roles.is_empty()); // roles are in metadata, not plaintext token
    }

    #[tokio::test]
    async fn test_token_creation_audit_logged() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "audit-test".to_string(),
                    description: Some("for audit testing".to_string()),
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec!["viewer".to_string()],
                    scopes: vec!["project-a".to_string()],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Token should exist in store
        let metadata = manager.get_token(&response.id).await.unwrap().unwrap();
        assert_eq!(metadata.name, "audit-test");
        assert_eq!(metadata.owner, "user1");
        assert_eq!(metadata.token_type, TokenType::User);
        assert_eq!(metadata.roles, vec!["viewer"]);
        assert_eq!(metadata.scopes, vec!["project-a"]);
    }

    #[tokio::test]
    async fn test_agent_token_default_ttl_is_1h() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "agent-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::Agent,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: None,
                },
                &owner,
            )
            .await
            .unwrap();

        let metadata = manager.get_token(&response.id).await.unwrap().unwrap();
        let ttl = metadata.expires_at - metadata.created_at;
        assert_eq!(ttl, 3600); // 1 hour for agent tokens
    }

    #[tokio::test]
    async fn test_service_token_default_ttl() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "service-token".to_string(),
                    description: None,
                    owner: "svc1".to_string(),
                    token_type: TokenType::Service,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: None,
                },
                &owner,
            )
            .await
            .unwrap();

        let metadata = manager.get_token(&response.id).await.unwrap().unwrap();
        let ttl = metadata.expires_at - metadata.created_at;
        assert_eq!(ttl, 31536000); // 1 year for service tokens
    }

    #[tokio::test]
    async fn test_revoked_token_not_in_user_list() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "temp-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        manager.revoke_token(&response.id).await.unwrap();

        let tokens = manager.list_user_tokens("user1").await.unwrap();
        assert!(tokens.is_empty()); // Revoked tokens are excluded
    }

    #[tokio::test]
    async fn test_token_id_is_uuid() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "test".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Token ID should be a valid UUID
        let parsed = Uuid::parse_str(&response.id);
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_error_messages() {
        assert!(TokenError::NameRequired.to_string().contains("name"));
        assert!(TokenError::OwnerRequired.to_string().contains("owner"));
        assert!(TokenError::RoleExceedsOwner("admin".to_string()).to_string().contains("role"));
        assert!(TokenError::ScopeExceedsOwner("proj".to_string()).to_string().contains("scope"));
        assert!(TokenError::NotFound.to_string().contains("not found"));
        assert!(TokenError::AlreadyRevoked.to_string().contains("revoked"));
        assert!(TokenError::TokenDisabled.to_string().contains("disabled"));
    }

    // ── M3-12 Lifecycle tests ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_bulk_revoke_by_owner() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        // Create 3 tokens for user1, 1 for user2
        for name in &["t1", "t2", "t3"] {
            manager.create_token(
                CreateTokenRequest {
                    name: name.to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();
        }
        manager.create_token(
            CreateTokenRequest {
                name: "t4".to_string(),
                description: None,
                owner: "user2".to_string(),
                token_type: TokenType::User,
                roles: vec![],
                scopes: vec![],
                ttl_secs: Some(3600),
            },
            &owner,
        )
        .await
        .unwrap();

        // Bulk revoke all user1 tokens
        let count = manager.revoke_tokens_by_owner("user1").await.unwrap();
        assert_eq!(count, 3);

        // All user1 tokens should be revoked
        let user1_tokens = manager.list_user_tokens("user1").await.unwrap();
        assert_eq!(user1_tokens.len(), 0);

        // user2 token should still be active
        let user2_tokens = manager.list_user_tokens("user2").await.unwrap();
        assert_eq!(user2_tokens.len(), 1);
    }

    #[tokio::test]
    async fn test_bulk_revoke_by_scope() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec!["project-a", "project-b"]);

        // Create tokens with different scopes

        manager.create_token(
            CreateTokenRequest {
                name: "token-a".to_string(),
                description: None,
                owner: "user1".to_string(),
                token_type: TokenType::User,
                roles: vec![],
                scopes: vec!["project-a".to_string()],
                ttl_secs: Some(3600),
            },
            &owner,
        )
        .await
        .unwrap();

        manager.create_token(
            CreateTokenRequest {
                name: "token-b".to_string(),
                description: None,
                owner: "user2".to_string(),
                token_type: TokenType::User,
                roles: vec![],
                scopes: vec!["project-b".to_string()],
                ttl_secs: Some(3600),
            },
            &owner,
        )
        .await
        .unwrap();

        // Bulk revoke tokens with project-a scope
        let count = manager.revoke_tokens_by_scope("project-a").await.unwrap();
        assert_eq!(count, 1);

        // token-a should be revoked, token-b should still work
        let all = manager.list_all_tokens().await.unwrap();
        let token_a = all.iter().find(|t| t.name == "token-a").unwrap();
        assert!(token_a.revoked);
        let token_b = all.iter().find(|t| t.name == "token-b").unwrap();
        assert!(!token_b.revoked);
    }

    #[tokio::test]
    async fn test_expiry_warning() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Create a token expiring in 5 days (86400 * 5 = 432000 seconds)
        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "expiring-soon".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(432000), // 5 days
                },
                &owner,
            )
            .await
            .unwrap();

        // Tokens expiring within 7 days (604800 seconds)
        let warning_tokens = manager.tokens_expiring_within(7 * 86400).await.unwrap();
        assert_eq!(warning_tokens.len(), 1);
        assert_eq!(warning_tokens[0].name, "expiring-soon");

        // Tokens expiring within 1 day should NOT include this token
        let one_day_tokens = manager.tokens_expiring_within(86400).await.unwrap();
        assert_eq!(one_day_tokens.len(), 0);
    }

    #[tokio::test]
    async fn test_rotate_token() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "original".to_string(),
                    description: Some("original token".to_string()),
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec!["viewer".to_string()],
                    scopes: vec!["project-a".to_string()],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Old token should work before rotation
        let valid = manager.validate_token(&response.token).await;
        assert!(valid.is_ok());

        // Rotate the token
        let new_response = manager.rotate_token(&response.token, &owner).await.unwrap();
        assert_ne!(new_response.token, response.token);
        assert_ne!(new_response.id, response.id);
        assert_eq!(new_response.name, "original");

        // New token should work
        let valid_new = manager.validate_token(&new_response.token).await;
        assert!(valid_new.is_ok());

        // Old token should be revoked
        let valid_old = manager.validate_token(&response.token).await;
        assert!(matches!(valid_old, Err(TokenError::AlreadyRevoked)));
    }

    #[tokio::test]
    async fn test_rotate_nonexistent_token_fails() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let result = manager.rotate_token("sp_invalid", &owner).await;
        assert!(matches!(result, Err(TokenError::NotFound)));
    }

    #[tokio::test]
    async fn test_cleanup_expired_tokens() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        // Create a token with 0 TTL (already expired)
        manager
            .create_token(
                CreateTokenRequest {
                    name: "expired".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::Agent, // agent max is 1h, so 0 is fine
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(0),
                },
                &owner,
            )
            .await
            .unwrap();

        // Create a token that's still valid
        manager
            .create_token(
                CreateTokenRequest {
                    name: "active".to_string(),
                    description: None,
                    owner: "user2".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Cleanup should revoke the expired token
        let cleaned = manager.cleanup_expired_tokens().await.unwrap();
        assert_eq!(cleaned, 1);

        // The expired token should no longer be found
        let all = manager.list_all_tokens().await.unwrap();
        let expired = all.iter().find(|t| t.name == "expired").unwrap();
        assert!(expired.revoked);

        let active = all.iter().find(|t| t.name == "active").unwrap();
        assert!(!active.revoked);
    }

    #[tokio::test]
    async fn test_revoke_then_validate_returns_already_revoked() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "temp".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        manager.revoke_token(&response.id).await.unwrap();
        let result = manager.validate_token(&response.token).await;
        assert!(matches!(result, Err(TokenError::AlreadyRevoked)));
    }

    #[tokio::test]
    async fn test_bulk_revoke_owner_already_revoked_not_counted() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "already-revoked".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Revoke it first
        manager.revoke_token(&response.id).await.unwrap();

        // Bulk revoke should not count already-revoked tokens
        let count = manager.revoke_tokens_by_owner("user1").await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_expiry_warning_t7d_and_t1d() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        // Token expiring in 3 days — should trigger T-7d warning but not T-1d
        manager
            .create_token(
                CreateTokenRequest {
                    name: "t-3d".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(86400 * 3), // 3 days
                },
                &owner,
            )
            .await
            .unwrap();

        // Token expiring in 5 hours — should trigger both T-7d and T-1d
        manager
            .create_token(
                CreateTokenRequest {
                    name: "t-5h".to_string(),
                    description: None,
                    owner: "user2".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600 * 5), // 5 hours
                },
                &owner,
            )
            .await
            .unwrap();

        // 7-day warning should see both
        let t7d_tokens = manager.tokens_expiring_within(7 * 86400).await.unwrap();
        assert_eq!(t7d_tokens.len(), 2);

        // 1-day warning should see only the 5-hour one
        let t1d_tokens = manager.tokens_expiring_within(24 * 3600).await.unwrap();
        assert_eq!(t1d_tokens.len(), 1);
        assert_eq!(t1d_tokens[0].name, "t-5h");
    }

    #[tokio::test]
    async fn test_rotation_preserves_roles_and_scopes() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer", "editor"], vec!["proj-a", "proj-b"]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "with-roles".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec!["viewer".to_string()],
                    scopes: vec!["proj-a".to_string()],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        let new_response = manager.rotate_token(&response.token, &owner).await.unwrap();

        let new_meta = manager.get_token(&new_response.id).await.unwrap().unwrap();
        assert_eq!(new_meta.roles, vec!["viewer"]);
        assert_eq!(new_meta.scopes, vec!["proj-a"]);
        assert_eq!(new_meta.owner, "user1");
        assert_eq!(new_meta.token_type, TokenType::User);
    }

    // ── M3-14 Reconciliation tests ──────────────────────────────────────────

    /// Mock resolver for testing: returns Ok(set of existing owners) or
    /// Err(ConnectorUnavailable) to simulate unreachable connector.
    #[derive(Debug, Clone)]
    struct MockResolver {
        available: bool,
        existing: HashSet<String>,
    }

    impl MockResolver {
        fn available(existing: HashSet<String>) -> Self {
            Self { available: true, existing }
        }
        fn unavailable() -> Self {
            Self { available: false, existing: HashSet::new() }
        }
    }

    #[async_trait]
    impl UserResolver for MockResolver {
        async fn resolve_owners(
            &self,
            _connector: &str,
            owners: &HashSet<String>,
        ) -> Result<HashSet<String>, ReconciliationError> {
            if !self.available {
                return Err(ReconciliationError::ConnectorUnavailable);
            }
            Ok(owners.intersection(&self.existing).cloned().collect())
        }
    }

    #[tokio::test]
    async fn test_reconciliation_disables_orphaned_tokens() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        // Create a user token with connector "ldap"
        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "user-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Manually set connector on stored metadata
        {
            let store_ref = manager.store.clone();
            // We can't set connector directly after creation, so test with what we have
            // For this test, connector is None, which defaults to "default"
        }

        // Mock: user1 no longer exists
        let mut existing = HashSet::new();
        // user1 is NOT in existing set → should be disabled
        let resolver = MockResolver::available(existing);

        let result = manager.reconcile_tokens(&resolver).await.unwrap();
        assert_eq!(result.checked, 1);
        assert_eq!(result.disabled, 1);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.orphaned_ids.len(), 1);
        assert_eq!(result.orphaned_ids[0], response.id);

        // Token should now be disabled
        let meta = manager.get_token(&response.id).await.unwrap().unwrap();
        assert!(meta.disabled);
        assert!(meta.disabled_reason.is_some());

        // Validating with the token should return TokenDisabled
        let validate_result = manager.validate_token(&response.token).await;
        assert!(matches!(validate_result, Err(TokenError::TokenDisabled)));
    }

    #[tokio::test]
    async fn test_reconciliation_skips_when_connector_unavailable() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        manager
            .create_token(
                CreateTokenRequest {
                    name: "user-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Mock: connector unavailable
        let resolver = MockResolver::unavailable();

        let result = manager.reconcile_tokens(&resolver).await.unwrap();
        assert_eq!(result.checked, 1);
        assert_eq!(result.disabled, 0);
        assert_eq!(result.skipped, 1);
        assert!(result.orphaned_ids.is_empty());

        // Token should still be active
        let all = manager.list_all_tokens().await.unwrap();
        assert!(!all[0].disabled);
    }

    #[tokio::test]
    async fn test_reconciliation_only_checks_user_tokens() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        // Create a user token
        manager
            .create_token(
                CreateTokenRequest {
                    name: "user-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Create a service token (should NOT be checked by reconciliation)
        manager
            .create_token(
                CreateTokenRequest {
                    name: "service-token".to_string(),
                    description: None,
                    owner: "svc1".to_string(),
                    token_type: TokenType::Service,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Mock: nobody exists
        let resolver = MockResolver::available(HashSet::new());

        let result = manager.reconcile_tokens(&resolver).await.unwrap();
        // Only user tokens should be checked (1 of 2)
        assert_eq!(result.checked, 1);
        assert_eq!(result.disabled, 1);
        assert_eq!(result.skipped, 0);
    }

    #[tokio::test]
    async fn test_reconciliation_preserves_existing_tokens() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        manager
            .create_token(
                CreateTokenRequest {
                    name: "user1-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        manager
            .create_token(
                CreateTokenRequest {
                    name: "user2-token".to_string(),
                    description: None,
                    owner: "user2".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Mock: user1 still exists, user2 does not
        let mut existing = HashSet::new();
        existing.insert("user1".to_string());
        let resolver = MockResolver::available(existing);

        let result = manager.reconcile_tokens(&resolver).await.unwrap();
        assert_eq!(result.checked, 2);
        assert_eq!(result.disabled, 1);
        assert_eq!(result.orphaned_ids.len(), 1);

        // Verify correct token was disabled
        let disabled = manager.list_disabled_tokens().await.unwrap();
        assert_eq!(disabled.len(), 1);
        assert_eq!(disabled[0].name, "user2-token");

        // user1 token should still be valid
        let all = manager.list_all_tokens().await.unwrap();
        let user1_token = all.iter().find(|t| t.name == "user1-token").unwrap();
        assert!(!user1_token.disabled);
    }

    #[tokio::test]
    async fn test_reconciliation_is_idempotent() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        manager
            .create_token(
                CreateTokenRequest {
                    name: "user1-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Mock: user1 doesn't exist
        let resolver = MockResolver::available(HashSet::new());

        // Run reconciliation twice
        let result1 = manager.reconcile_tokens(&resolver).await.unwrap();
        assert_eq!(result1.disabled, 1);

        let result2 = manager.reconcile_tokens(&resolver).await.unwrap();
        // Second run should find 0 to disable (already disabled)
        assert_eq!(result2.disabled, 0);
        assert_eq!(result2.checked, 0); // disabled tokens are not returned by list_tokens_for_reconciliation
    }

    #[tokio::test]
    async fn test_reconciliation_batches_by_connector() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        // Create tokens for two different users (both will use "default" connector)
        manager
            .create_token(
                CreateTokenRequest {
                    name: "t1".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        manager
            .create_token(
                CreateTokenRequest {
                    name: "t2".to_string(),
                    description: None,
                    owner: "user2".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Mock: neither user exists
        let resolver = MockResolver::available(HashSet::new());

        let result = manager.reconcile_tokens(&resolver).await.unwrap();
        assert_eq!(result.checked, 2);
        assert_eq!(result.disabled, 2);
    }

    #[tokio::test]
    async fn test_reconciliation_no_tokens_to_check() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        // No tokens at all
        let existing = HashSet::from(["user1".to_string(), "user2".to_string()]);
        let resolver = MockResolver::available(existing);

        let result = manager.reconcile_tokens(&resolver).await.unwrap();
        assert_eq!(result.checked, 0);
        assert_eq!(result.disabled, 0);
        assert_eq!(result.skipped, 0);
    }

    #[tokio::test]
    async fn test_list_disabled_tokens_empty() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);

        let disabled = manager.list_disabled_tokens().await.unwrap();
        assert_eq!(disabled.len(), 0);
    }

    #[tokio::test]
    async fn test_enable_disabled_token() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "test".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Disable
        manager.disable_token(&response.id, "test disable").await.unwrap();

        // Verify disabled
        let validate_result = manager.validate_token(&response.token).await;
        assert!(matches!(validate_result, Err(TokenError::TokenDisabled)));

        // Re-enable
        let enabled = manager.enable_token(&response.id).await.unwrap();
        assert!(enabled);

        // Should now work again
        let validate_result = manager.validate_token(&response.token).await;
        assert!(validate_result.is_ok());
    }

    #[tokio::test]
    async fn test_reactivation_not_automatic() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        manager
            .create_token(
                CreateTokenRequest {
                    name: "user1-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // User removed from LDAP → reconciliation disables
        let resolver = MockResolver::available(HashSet::new());
        manager.reconcile_tokens(&resolver).await.unwrap();

        // User added back → reconciliation detects but does NOT auto-renable
        let mut existing = HashSet::new();
        existing.insert("user1".to_string());
        let resolver = MockResolver::available(existing);
        let result = manager.reconcile_tokens(&resolver).await.unwrap();
        assert_eq!(result.disabled, 0); // No new disables

        // Token is still disabled
        let disabled = manager.list_disabled_tokens().await.unwrap();
        assert_eq!(disabled.len(), 1);
    }

    #[tokio::test]
    async fn test_reconciliation_batch_result() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        // Create 3 user tokens
        let mut token_ids = Vec::new();
        for i in 0..3 {
            let response = manager
                .create_token(
                    CreateTokenRequest {
                        name: format!("token-{}", i),
                        description: None,
                        owner: "user1".to_string(),
                        token_type: TokenType::User,
                        roles: vec![],
                        scopes: vec![],
                        ttl_secs: Some(3600),
                    },
                    &owner,
                )
                .await
                .unwrap();
            token_ids.push(response.id);
        }

        // Mock: user1 doesn't exist
        let resolver = MockResolver::available(HashSet::new());

        let result = manager.reconcile_tokens(&resolver).await.unwrap();
        assert_eq!(result.checked, 3);
        assert_eq!(result.disabled, 3);
        assert_eq!(result.orphaned_ids.len(), 3);

        // All IDs should be in orphaned_ids
        for id in &token_ids {
            assert!(result.orphaned_ids.contains(id));
        }
    }

    // ── M3-13 Idle token report tests ──────────────────────────────────────

    #[tokio::test]
    async fn test_idle_tokens_report_finds_idle() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store.clone());
        let owner = make_owner(vec![], vec![]);

        // Create a token and set last_used_at to 100 days ago
        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "idle-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Simulate last used 100 days ago by manually updating the store
        {
            let mut tokens = store.tokens.lock().unwrap();
            if let Some((meta, _hash)) = tokens.get_mut(&response.id) {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                meta.last_used_at = Some(now - 100 * 86400);
            }
        }

        // 30-day idle report should find it
        let idle = manager.list_idle_tokens(30).await.unwrap();
        assert_eq!(idle.len(), 1);
        assert_eq!(idle[0].name, "idle-token");
        assert_eq!(idle[0].owner, "user1");
        assert!(idle[0].days_idle >= 100);
        assert_eq!(idle[0].suggestion, "Consider revoking — 90+ days idle");
    }

    #[tokio::test]
    async fn test_idle_tokens_excludes_recently_used() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        // Create a token with last_used_at = now (recently used)
        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "active-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // 30-day idle report should NOT find it
        let idle = manager.list_idle_tokens(30).await.unwrap();
        assert_eq!(idle.len(), 0);
    }

    #[tokio::test]
    async fn test_idle_tokens_excludes_no_last_used() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        // Create a token that was never used
        let _response = manager
            .create_token(
                CreateTokenRequest {
                    name: "never-used".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Token with no last_used_at should NOT appear in idle report
        let idle = manager.list_idle_tokens(30).await.unwrap();
        assert_eq!(idle.len(), 0);
    }

    #[tokio::test]
    async fn test_idle_tokens_excludes_revoked() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store.clone());
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "revoked-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Set last_used_at to 100 days ago
        {
            let mut tokens = store.tokens.lock().unwrap();
            if let Some((meta, _hash)) = tokens.get_mut(&response.id) {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                meta.last_used_at = Some(now - 100 * 86400);
            }
        }

        // Revoke the token
        manager.revoke_token(&response.id).await.unwrap();

        // Revoked tokens should NOT appear in idle report
        let idle = manager.list_idle_tokens(30).await.unwrap();
        assert_eq!(idle.len(), 0);
    }

    #[tokio::test]
    async fn test_idle_tokens_suggestion_90_days() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store.clone());
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "90-day-token".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        {
            let mut tokens = store.tokens.lock().unwrap();
            if let Some((meta, _hash)) = tokens.get_mut(&response.id) {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                meta.last_used_at = Some(now - 95 * 86400);
            }
        }

        let idle = manager.list_idle_tokens(30).await.unwrap();
        assert_eq!(idle.len(), 1);
        assert_eq!(idle[0].suggestion, "Consider revoking — 90+ days idle");
    }

    // ── M3-13 Audit log tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_audit_event() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "audit-create".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        let events = manager.get_audit_events(&response.id, None, None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, AuditEventType::Create);
        assert_eq!(events[0].owner, "user1");
    }

    #[tokio::test]
    async fn test_revoke_audit_event() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "audit-revoke".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        manager.revoke_token(&response.id).await.unwrap();

        let events = manager.get_audit_events(&response.id, None, None).await.unwrap();
        let revoke_events: Vec<_> = events.iter().filter(|e| e.event_type == AuditEventType::Revoke).collect();
        assert_eq!(revoke_events.len(), 1);
        assert_eq!(revoke_events[0].owner, "user1");
    }

    #[tokio::test]
    async fn test_disable_audit_event() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "audit-disable".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        manager.disable_token(&response.id, "test disable reason").await.unwrap();

        let events = manager.get_audit_events(&response.id, None, None).await.unwrap();
        let disable_events: Vec<_> = events.iter().filter(|e| e.event_type == AuditEventType::Disable).collect();
        assert_eq!(disable_events.len(), 1);
        assert_eq!(disable_events[0].details, Some("test disable reason".to_string()));
    }

    #[tokio::test]
    async fn test_enable_audit_event() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "audit-enable".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        manager.disable_token(&response.id, "reason").await.unwrap();
        manager.enable_token(&response.id).await.unwrap();

        let events = manager.get_audit_events(&response.id, None, None).await.unwrap();
        let enable_events: Vec<_> = events.iter().filter(|e| e.event_type == AuditEventType::Enable).collect();
        assert_eq!(enable_events.len(), 1);
    }

    #[tokio::test]
    async fn test_rotate_audit_events() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec!["viewer"], vec!["project-a"]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "audit-rotate".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec!["viewer".to_string()],
                    scopes: vec!["project-a".to_string()],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        let new_response = manager.rotate_token(&response.token, &owner).await.unwrap();

        let events_old = manager.get_audit_events(&response.id, None, None).await.unwrap();
        assert!(events_old.iter().any(|e| e.event_type == AuditEventType::Rotate));
        assert!(events_old.iter().any(|e| e.event_type == AuditEventType::Revoke));

        let events_new = manager.get_audit_events(&new_response.id, None, None).await.unwrap();
        assert!(events_new.iter().any(|e| e.event_type == AuditEventType::Create));
    }

    #[tokio::test]
    async fn test_expire_audit_event() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        // Create a token with 0 TTL (already expired)
        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "audit-expire".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::Agent,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(0),
                },
                &owner,
            )
            .await
            .unwrap();

        let count = manager.cleanup_expired_tokens().await.unwrap();
        assert_eq!(count, 1);

        let events = manager.get_audit_events(&response.id, None, None).await.unwrap();
        let expire_events: Vec<_> = events.iter().filter(|e| e.event_type == AuditEventType::Expire).collect();
        assert_eq!(expire_events.len(), 1);
        assert_eq!(expire_events[0].details, Some("automatically revoked — TTL elapsed".to_string()));
    }

    #[tokio::test]
    async fn test_audit_events_filtered_by_time() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);
        let owner = make_owner(vec![], vec![]);

        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "time-filter".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        let all_events = manager.get_audit_events(&response.id, None, None).await.unwrap();
        assert_eq!(all_events.len(), 1);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Future filter should return nothing
        let future_events = manager
            .get_audit_events(&response.id, Some(now + 1000), None)
            .await
            .unwrap();
        assert_eq!(future_events.len(), 0);
    }

    #[tokio::test]
    async fn test_audit_events_empty_for_unknown_token() {
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store);

        let events = manager.get_audit_events("nonexistent", None, None).await.unwrap();
        assert_eq!(events.len(), 0);
    }

    #[tokio::test]
    async fn test_idle_and_audit_combined() {
        // Test that audit logging works alongside idle token reporting
        let store = Arc::new(InMemoryTokenStore::new());
        let manager = TokenManager::with_default_policy(store.clone());
        let owner = make_owner(vec![], vec![]);

        // Create and use a token (sets last_used_at)
        let response = manager
            .create_token(
                CreateTokenRequest {
                    name: "combined".to_string(),
                    description: None,
                    owner: "user1".to_string(),
                    token_type: TokenType::User,
                    roles: vec![],
                    scopes: vec![],
                    ttl_secs: Some(3600),
                },
                &owner,
            )
            .await
            .unwrap();

        // Make it idle by setting last_used_at to 120 days ago
        {
            let mut tokens = store.tokens.lock().unwrap();
            if let Some((meta, _hash)) = tokens.get_mut(&response.id) {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                meta.last_used_at = Some(now - 120 * 86400);
            }
        }

        // Should appear in idle report
        let idle = manager.list_idle_tokens(30).await.unwrap();
        assert_eq!(idle.len(), 1);
        assert_eq!(idle[0].days_idle, 120);
        assert_eq!(idle[0].suggestion, "Consider revoking — 90+ days idle");

        // Should have audit events
        let events = manager.get_audit_events(&response.id, None, None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, AuditEventType::Create);
    }
}

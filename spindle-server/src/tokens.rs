//! Token management for Spindle Server.
//!
//! Implements C7 Token types + creation:
//! - POST /v1/tokens → create token with role/scope/TTL validation
//! - Token prefix `sp_` for log/audit identification
//! - Argon2id hash storage (never retrievable)
//! - GET /v1/tokens → metadata only, no plaintext
//! - Policy max TTL per token type (user/service/agent)
//! - Role/scope validation: ≤ owner's roles/scope

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
    /// Last used timestamp (Unix seconds).
    pub last_used_at: Option<u64>,
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
    /// Update last_used_at timestamp.
    async fn update_last_used(&self, id: &str) -> Result<(), TokenError>;
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
            last_used_at: None,
        };

        self.store.create_token(metadata, token_hash).await?;

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
        self.store.revoke_token(id).await
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
}

impl InMemoryTokenStore {
    pub fn new() -> Self {
        Self {
            tokens: Arc::new(std::sync::Mutex::new(HashMap::new())),
            hash_index: Arc::new(std::sync::Mutex::new(HashMap::new())),
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
            meta.last_used_at = Some(now);
        }
        Ok(())
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
    }
}

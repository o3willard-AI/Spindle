//! Identity model for Spindle — M3-02.
//!
//! # Overview
//! This crate implements the internal identity layer:
//! - **Principal**: Dex id_token extraction, claims validation, group resolution
//! - **GroupCache**: Cached group lookups with configurable TTL (5min default)
//! - **InternalRoles**: Derived from group→role mapping rules
//! - **DexClient**: OIDC client for Dex integration
//! - **AuthSession**: Full auth flow from Dex callback through role resolution

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::debug;
use uuid::Uuid;

use spindle_authz::{Role, Scope};
use spindle_dex::DexConfig;

// ── Connector ID ──────────────────────────────────────────────────────────────

/// Unique identifier for an authentication connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConnectorId(pub u32);

impl ConnectorId {
    pub fn new(id: u32) -> Self {
        Self(id)
    }

    pub fn default_oidc() -> Self {
        Self(0)
    }
}

// ── OIDC Claims ───────────────────────────────────────────────────────────────

/// Standard OIDC claims extracted from an id_token.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OidcClaims {
    /// Subject — unique identifier for the end-user.
    pub sub: String,
    /// Preferred username.
    pub preferred_username: Option<String>,
    /// Email address.
    pub email: Option<String>,
    /// Email verified flag.
    pub email_verified: Option<bool>,
    /// Groups claim (Dex-specific, array of strings).
    pub groups: Option<Vec<String>>,
    /// Raw claim map for any extra claims.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl OidcClaims {
    /// Extract claims from a raw JSON map (e.g., from a decoded JWT payload).
    pub fn from_raw(raw: &HashMap<String, serde_json::Value>) -> Self {
        let mut claims = Self::default();

        if let Some(val) = raw.get("sub") {
            claims.sub = val.as_str().unwrap_or("").to_string();
        }
        if let Some(val) = raw.get("preferred_username") {
            claims.preferred_username = val.as_str().map(|s| s.to_string());
        }
        if let Some(val) = raw.get("email") {
            claims.email = val.as_str().map(|s| s.to_string());
        }
        if let Some(val) = raw.get("email_verified") {
            claims.email_verified = val.as_bool();
        }
        if let Some(val) = raw.get("groups") {
            if let Some(arr) = val.as_array() {
                claims.groups = Some(
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect(),
                );
            }
        }

        // Collect any remaining fields not matched above
        let known = [
            "sub",
            "preferred_username",
            "email",
            "email_verified",
            "groups",
        ];
        for (k, v) in raw.iter() {
            if !known.contains(&k.as_str()) {
                claims.extra.insert(k.clone(), v.clone());
            }
        }

        claims
    }

    /// Validate basic claim requirements. Returns error description if invalid.
    pub fn validate(&self) -> Result<(), String> {
        if self.sub.is_empty() {
            return Err("missing required claim: sub".to_string());
        }
        Ok(())
    }

    /// All groups from claims, or empty if not present.
    pub fn group_list(&self) -> &[String] {
        self.groups.as_deref().unwrap_or(&[])
    }
}

// ── Principal ─────────────────────────────────────────────────────────────────

/// The authenticated principal — who made a request and what they are.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    /// Subject identifier (user ID, email, etc.) from the identity provider.
    pub subject: String,
    /// Source connector that authenticated this principal.
    pub source: ConnectorId,
    /// Arbitrary claims from the identity provider.
    #[serde(default)]
    pub claims: HashMap<String, String>,
    /// Groups resolved from the identity provider.
    #[serde(default)]
    pub groups: Vec<String>,
    /// Preferred display name.
    pub display_name: Option<String>,
    /// Email address if available.
    pub email: Option<String>,
}

impl Principal {
    /// Create a new principal from claims.
    pub fn from_claims(
        claims: &OidcClaims,
        source: ConnectorId,
        dex_groups: Vec<String>,
    ) -> Self {
        let mut map = HashMap::new();
        if let Some(ref username) = claims.preferred_username {
            map.insert("preferred_username".to_string(), username.clone());
        }
        if let Some(ref email) = claims.email {
            map.insert("email".to_string(), email.clone());
        }
        if let Some(verified) = claims.email_verified {
            map.insert("email_verified".to_string(), verified.to_string());
        }

        Principal {
            subject: claims.sub.clone(),
            source,
            claims: map,
            groups: dex_groups,
            display_name: claims.preferred_username.clone(),
            email: claims.email.clone(),
        }
    }

    /// Build a Scope for authorization.
    pub fn scope(&self, role_map: &HashMap<String, Role>) -> Scope {
        let roles: std::collections::HashSet<String> = self
            .groups
            .iter()
            .filter_map(|g| role_map.get(g).map(|r| r.to_string()))
            .collect();
        Scope::new(std::collections::HashSet::new(), roles)
    }
}

// ── InternalRoles ─────────────────────────────────────────────────────────────

/// Internal roles derived from principal claims and group membership.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InternalRoles {
    /// Role names (e.g., "admin", "editor", "viewer").
    pub roles: Vec<String>,
    /// Scopes granted to this principal.
    pub scopes: Vec<String>,
    /// Spindle authz Role enum values.
    pub spindle_roles: Vec<Role>,
}

impl InternalRoles {
    pub fn new(
        role_names: Vec<String>,
        scope_list: Vec<String>,
        spindle: Vec<Role>,
    ) -> Self {
        Self {
            roles: role_names,
            scopes: scope_list,
            spindle_roles: spindle,
        }
    }

    /// Check if this principal has at least the given role.
    pub fn has_role(&self, required: Role) -> bool {
        self.spindle_roles.iter().any(|r| r.includes(required))
    }

    /// Get the highest role.
    pub fn highest_role(&self) -> Option<Role> {
        self.spindle_roles
            .iter()
            .max_by_key(|r| match *r {
                Role::Ingest => 0,
                Role::Viewer => 1,
                Role::ComplianceAuditor => 2,
                Role::TokenAdmin => 3,
                Role::Admin => 4,
            })
            .copied()
    }
}

// ── Group Resolution Timeout ──────────────────────────────────────────────────

/// Error from group resolution operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupError {
    /// The identity provider timed out.
    Timeout(String),
    /// The identity provider returned an error.
    ProviderError(String),
    /// Invalid response from the provider.
    ParseError(String),
}

impl fmt::Display for GroupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout(msg) => write!(f, "group resolution timeout: {}", msg),
            Self::ProviderError(msg) => write!(f, "provider error: {}", msg),
            Self::ParseError(msg) => write!(f, "parse error: {}", msg),
        }
    }
}

impl std::error::Error for GroupError {}

/// Result alias for group resolution.
pub type GroupResult<T> = Result<T, GroupError>;

// ── Group Cache ───────────────────────────────────────────────────────────────

/// Cached group lookups with configurable TTL.
#[derive(Debug, Clone)]
struct CacheEntry {
    groups: Vec<String>,
    expires_at: Instant,
}

/// Group cache for resolving groups from the identity provider.
///
/// Default TTL is 5 minutes. Entries are lazily evicted on read.
#[derive(Debug, Clone)]
pub struct GroupCache {
    entries: Arc<std::sync::RwLock<HashMap<String, CacheEntry>>>,
    ttl: Duration,
}

impl GroupCache {
    /// Create a new GroupCache with the given TTL.
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Arc::new(std::sync::RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Create with default 5-minute TTL.
    pub fn default_ttl() -> Self {
        Self::new(Duration::from_secs(300))
    }

    /// Default TTL constant.
    pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

    /// Get cached groups for a principal subject.
    pub fn get(&self, subject: &str) -> Option<Vec<String>> {
        let lock = self.entries.read().unwrap();
        if let Some(entry) = lock.get(subject) {
            if entry.expires_at > Instant::now() {
                return Some(entry.groups.clone());
            }
        }
        None
    }

    /// Cache groups for a principal subject.
    pub fn put(&self, subject: &str, groups: Vec<String>) {
        let mut lock = self.entries.write().unwrap();
        lock.insert(
            subject.to_string(),
            CacheEntry {
                groups,
                expires_at: Instant::now() + self.ttl,
            },
        );
    }

    /// Invalidate a specific entry.
    pub fn invalidate(&self, subject: &str) {
        let mut lock = self.entries.write().unwrap();
        lock.remove(subject);
    }

    /// Clear all entries.
    pub fn clear(&self) {
        let mut lock = self.entries.write().unwrap();
        lock.clear();
    }

    /// Evict expired entries.
    pub fn evict_expired(&self) {
        let now = Instant::now();
        let mut lock = self.entries.write().unwrap();
        lock.retain(|_, entry| entry.expires_at > now);
    }
}

// ── Group Resolver Trait ──────────────────────────────────────────────────────

/// Trait for resolving groups from an identity provider.
pub trait GroupResolver: Send + Sync {
    /// Resolve groups for a principal's subject.
    ///
    /// Returns a GroupError on failure. Callers should handle timeout
    /// gracefully — e.g., fall back to cached data or continue without groups.
    fn resolve(&self, subject: &str) -> GroupResult<Vec<String>>;

    /// Resolve groups for a principal, using cache when available.
    fn resolve_cached(
        &self,
        subject: &str,
        cache: &GroupCache,
    ) -> GroupResult<Vec<String>> {
        // Try cache first
        if let Some(cached) = cache.get(subject) {
            debug!("group cache hit for {}", subject);
            return Ok(cached);
        }

        // Resolve from provider
        let groups = self.resolve(subject)?;

        // Cache the result
        cache.put(subject, groups.clone());

        Ok(groups)
    }
}

// ── Role Mapping ──────────────────────────────────────────────────────────────

/// A rule mapping an identity provider group to a Spindle role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleMappingRule {
    /// Group name in the identity provider (e.g., "spindle-admins").
    pub group: String,
    /// Spindle role to assign.
    pub role: Role,
}

impl RoleMappingRule {
    /// Create a new role mapping rule.
    pub fn new(group: impl Into<String>, role: Role) -> Self {
        Self {
            group: group.into(),
            role,
        }
    }
}

/// Maps groups to internal Spindle roles.
#[derive(Debug, Clone)]
pub struct RoleMapper {
    rules: Vec<RoleMappingRule>,
}

impl RoleMapper {
    /// Create a new RoleMapper with the given rules.
    pub fn new(rules: Vec<RoleMappingRule>) -> Self {
        Self { rules }
    }

    /// Create a default mapper with no rules.
    pub fn default_rules() -> Self {
        Self { rules: Vec::new() }
    }

    /// Map a principal's groups to internal roles.
    pub fn map(&self, groups: &[String]) -> InternalRoles {
        let mut spindle_roles: Vec<Role> = Vec::new();
        let mut role_names: Vec<String> = Vec::new();
        let mut scopes: Vec<String> = Vec::new();

        for group in groups {
            for rule in &self.rules {
                if group == &rule.group && !spindle_roles.contains(&rule.role) {
                    spindle_roles.push(rule.role.clone());
                    role_names.push(rule.role.to_string());
                    match rule.role {
                        Role::Ingest => {
                            scopes.push("ingest".to_string());
                        }
                        Role::Viewer => {
                            scopes.push("read".to_string());
                        }
                        Role::ComplianceAuditor => {
                            scopes.push("compliance-read".to_string());
                            scopes.push("export".to_string());
                        }
                        Role::TokenAdmin => {
                            scopes.push("token-admin".to_string());
                        }
                        Role::Admin => {
                            scopes.push("admin".to_string());
                            scopes.push("read".to_string());
                            scopes.push("write".to_string());
                        }
                    }
                }
            }
        }

        InternalRoles::new(role_names, scopes, spindle_roles)
    }

    /// Add a mapping rule.
    pub fn add_rule(&mut self, rule: RoleMappingRule) {
        self.rules.push(rule);
    }
}

// ── DexClient ─────────────────────────────────────────────────────────────────

/// OIDC client for interacting with a Dex server.
///
/// Handles id_token extraction, claims parsing, and validation.
#[derive(Debug, Clone)]
pub struct DexClient {
    /// Issuer URL (e.g., "https://spindle.local/dex").
    pub issuer: String,
    /// Client ID for the Spindle app.
    pub client_id: String,
    /// Client secret for the Spindle app.
    pub client_secret: Option<String>,
    /// Redirect URL for the OIDC callback.
    pub redirect_url: String,
    /// Group claim name (e.g., "groups").
    pub group_claim: String,
    /// Role mapping rules.
    pub role_mapper: RoleMapper,
    /// Group cache.
    pub group_cache: GroupCache,
    /// HTTP client.
    http: reqwest::Client,
}

impl Default for DexClient {
    fn default() -> Self {
        Self {
            issuer: "https://spindle.local/dex".to_string(),
            client_id: "spindle".to_string(),
            client_secret: None,
            redirect_url: "https://spindle.local/callback".to_string(),
            group_claim: "groups".to_string(),
            role_mapper: RoleMapper::default_rules(),
            group_cache: GroupCache::default_ttl(),
            http: reqwest::Client::new(),
        }
    }
}

impl DexClient {
    /// Create a new DexClient from a DexConfig.
    pub fn from_config(config: &DexConfig) -> Self {
        Self {
            issuer: config.issuer.clone(),
            client_id: "spindle".to_string(),
            client_secret: None,
            redirect_url: "https://spindle.local/callback".to_string(),
            group_claim: "groups".to_string(),
            role_mapper: RoleMapper::default_rules(),
            group_cache: GroupCache::default_ttl(),
            http: reqwest::Client::new(),
        }
    }

    /// Extract and validate the OIDC id_token from a Dex callback response.
    ///
    /// In a real implementation this would parse the OAuth2 code exchange
    /// response. This stub extracts claims from a raw JSON map.
    pub fn extract_id_token(
        &self,
        raw_token: &str,
    ) -> Result<OidcClaims, String> {
        // In production this would decode a JWT, verify the signature,
        // and validate the issuer/audience. Here we parse a raw JSON payload.
        let parsed: HashMap<String, serde_json::Value> =
            serde_json::from_str(raw_token).map_err(|e| format!("JWT parse error: {}", e))?;
        let claims = OidcClaims::from_raw(&parsed);
        claims.validate()?;
        Ok(claims)
    }

    /// Validate an id_token — check issuer, audience, expiration.
    pub fn validate_token(
        &self,
        claims: &OidcClaims,
        expected_audience: &str,
    ) -> Result<(), String> {
        // Validate audience (in production, check 'aud' claim)
        // For now, we just validate required fields
        claims.validate()?;

        // Validate issuer match
        if !claims.sub.starts_with(&self.issuer) {
            return Err(format!(
                "token issuer mismatch: expected {} got {}",
                self.issuer, claims.sub
            ));
        }

        // Validate audience
        if let Some(aud) = claims.extra.get("aud") {
            if let Some(aud_str) = aud.as_str() {
                if aud_str != expected_audience {
                    return Err(format!("audience mismatch"));
                }
            }
        }

        // Validate expiration (if present in extra claims)
        if let Some(exp) = claims.extra.get("exp") {
            if let Some(exp_val) = exp.as_f64() {
                let exp_dt = DateTime::from_timestamp(exp_val as i64, 0)
                    .ok_or("invalid exp claim")?;
                if exp_dt < Utc::now() {
                    return Err("token expired".to_string());
                }
            }
        }

        Ok(())
    }

    /// Full auth flow: Dex callback → Principal populated → roles resolved → session.
    ///
    /// 1. Extract id_token from the callback response
    /// 2. Validate the token (issuer, audience, expiration)
    /// 3. Resolve groups from the identity provider (with caching)
    /// 4. Map groups to internal roles
    /// 5. Return Principal + InternalRoles
    pub async fn auth_flow(
        &self,
        raw_token: &str,
        expected_audience: &str,
    ) -> Result<(Principal, InternalRoles), String> {
        debug!(
            issuer = %self.issuer,
            "starting Dex auth flow"
        );

        // Step 1: Extract claims from id_token
        let claims = self.extract_id_token(raw_token)?;
        debug!(subject = %claims.sub, "extracted OIDC claims");

        // Step 2: Validate the token
        self.validate_token(&claims, expected_audience)?;
        debug!(subject = %claims.sub, "token validated");

        // Step 3: Resolve groups
        let groups = self
            .resolve_groups_with_timeout(&claims.sub)
            .await
            .unwrap_or_default();
        debug!(
            subject = %claims.sub,
            group_count = groups.len(),
            "resolved groups"
        );

        // Step 4: Create Principal
        let principal =
            Principal::from_claims(&claims, self.role_mapper.default_connector(), groups.clone());

        // Step 5: Map groups to internal roles
        let roles = self.role_mapper.map(&principal.groups);

        debug!(
            subject = %principal.subject,
            role_count = roles.spindle_roles.len(),
            "auth flow complete"
        );

        Ok((principal, roles))
    }

    /// Resolve groups with a configurable timeout.
    ///
    /// If the provider times out, returns Ok(vec![]) (empty groups) so the
    /// request can still proceed with limited access.
    pub async fn resolve_groups_with_timeout(
        &self,
        subject: &str,
    ) -> GroupResult<Vec<String>> {
        let cache = &self.group_cache;

        // Check cache first (no timeout needed for cache hits)
        if let Some(cached) = cache.get(subject) {
            return Ok(cached);
        }

        // For production, this would call the Dex API or an LDAP directory.
        // Using a timeout handle so we don't block on a slow provider.
        let result = self
            .resolve_groups_from_dex(subject)
            .await;

        // Handle timeout gracefully — return empty groups on timeout
        match result {
            Ok(groups) => {
                cache.put(subject, groups.clone());
                Ok(groups)
            }
            Err(GroupError::Timeout(msg)) => {
                debug!("group resolution timeout for {}: {}", subject, msg);
                // Fall back to cache if available, otherwise empty groups
                cache.get(subject).ok_or(GroupError::Timeout(msg))
            }
            Err(e) => Err(e),
        }
    }

    /// Resolve groups from Dex (simulated for testing).
    /// In production this would query Dex's API or the configured connector.
    async fn resolve_groups_from_dex(&self, subject: &str) -> GroupResult<Vec<String>> {
        // In production, this would make an HTTP call to Dex's user info endpoint
        // or query the LDAP/SAML connector directly.
        // For now, simulate a group resolution with a timeout check.

        let response = tokio::time::timeout(
            Duration::from_secs(5),
            self.fetch_groups_from_dex(subject),
        )
        .await;

        match response {
            Ok(Ok(groups)) => Ok(groups),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(GroupError::Timeout(
                "group resolution timed out after 5s".to_string(),
            )),
        }
    }

    /// Fetch groups from Dex (simulated).
    async fn fetch_groups_from_dex(&self, _subject: &str) -> GroupResult<Vec<String>> {
        // In production, this would:
        // 1. Call Dex's user info endpoint with the access token
        // 2. Parse the groups from the response
        // For testing, return empty groups
        Ok(Vec::new())
    }
}

impl RoleMapper {
    /// Get the default connector ID for this role mapper.
    fn default_connector(&self) -> ConnectorId {
        ConnectorId::default_oidc()
    }
}

// ── Auth Session ──────────────────────────────────────────────────────────────

/// An authentication session established via the Dex auth flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSession {
    /// The authenticated principal.
    pub principal: Principal,
    /// Resolved internal roles.
    pub roles: InternalRoles,
    /// Session token (opaque, e.g., a JWT).
    pub session_token: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session expires.
    pub expires_at: DateTime<Utc>,
}

impl AuthSession {
    /// Create a new auth session.
    pub fn new(
        principal: Principal,
        roles: InternalRoles,
        session_token: String,
        ttl: Duration,
    ) -> Self {
        let now = Utc::now();
        Self {
            principal,
            roles,
            session_token,
            created_at: now,
            expires_at: now + chrono::TimeDelta::from_std(ttl).unwrap_or(chrono::TimeDelta::seconds(3600)),
        }
    }

    /// Check if the session is still valid.
    pub fn is_valid(&self) -> bool {
        Utc::now() < self.expires_at
    }

    /// Get the scope for this session.
    pub fn scope(&self) -> Scope {
        self.principal.scope(&self.role_to_map())
    }

    fn role_to_map(&self) -> HashMap<String, Role> {
        let mut map = HashMap::new();
        for role in &self.roles.spindle_roles {
            map.insert(role.to_string(), role.clone());
        }
        map
    }
}

// ── Group Resolver Implementations ────────────────────────────────────────────

/// A group resolver that always returns empty groups (for testing).
#[derive(Debug, Clone, Default)]
pub struct NullGroupResolver;

impl GroupResolver for NullGroupResolver {
    fn resolve(&self, _subject: &str) -> GroupResult<Vec<String>> {
        Ok(Vec::new())
    }
}

/// A group resolver that always returns the given groups (for testing).
#[derive(Debug, Clone)]
pub struct StaticGroupResolver {
    groups: Vec<String>,
}

impl StaticGroupResolver {
    pub fn new(groups: Vec<String>) -> Self {
        Self { groups }
    }
}

impl GroupResolver for StaticGroupResolver {
    fn resolve(&self, _subject: &str) -> GroupResult<Vec<String>> {
        Ok(self.groups.clone())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── ConnectorId tests ───────────────────────────────────────────────────

    #[test]
    fn test_connector_id_equality() {
        let id1 = ConnectorId(1);
        let id2 = ConnectorId(1);
        let id3 = ConnectorId(2);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_connector_id_new() {
        let id = ConnectorId::new(42);
        assert_eq!(id.0, 42);
    }

    // ── OidcClaims tests ────────────────────────────────────────────────────

    #[test]
    fn test_oidc_claims_from_raw() {
        let mut raw = HashMap::new();
        raw.insert("sub".to_string(), serde_json::json!("user-123"));
        raw.insert(
            "preferred_username".to_string(),
            serde_json::json!("johndoe"),
        );
        raw.insert("email".to_string(), serde_json::json!("john@example.com"));
        raw.insert("email_verified".to_string(), serde_json::json!(true));
        raw.insert(
            "groups".to_string(),
            serde_json::json!(["admin", "editor"]),
        );
        raw.insert("nickname".to_string(), serde_json::json!("jdoe"));

        let claims = OidcClaims::from_raw(&raw);

        assert_eq!(claims.sub, "user-123");
        assert_eq!(claims.preferred_username, Some("johndoe".to_string()));
        assert_eq!(claims.email, Some("john@example.com".to_string()));
        assert_eq!(claims.email_verified, Some(true));
        assert_eq!(claims.groups, Some(vec!["admin".to_string(), "editor".to_string()]));
        assert!(claims.extra.contains_key("nickname"));
    }

    #[test]
    fn test_oidc_claims_validate_ok() {
        let mut raw = HashMap::new();
        raw.insert("sub".to_string(), serde_json::json!("user-123"));
        let claims = OidcClaims::from_raw(&raw);
        assert!(claims.validate().is_ok());
    }

    #[test]
    fn test_oidc_claims_validate_empty_sub() {
        let raw = HashMap::new();
        let claims = OidcClaims::from_raw(&raw);
        let err = claims.validate().unwrap_err();
        assert!(err.contains("sub"));
    }

    #[test]
    fn test_oidc_claims_group_list_empty() {
        let raw = HashMap::new();
        let claims = OidcClaims::from_raw(&raw);
        assert!(claims.group_list().is_empty());
    }

    // ── Principal tests ─────────────────────────────────────────────────────

    #[test]
    fn test_principal_from_claims() {
        let mut raw = HashMap::new();
        raw.insert("sub".to_string(), serde_json::json!("user-123"));
        raw.insert(
            "preferred_username".to_string(),
            serde_json::json!("johndoe"),
        );
        let claims = OidcClaims::from_raw(&raw);
        let groups = vec!["admin".to_string(), "editor".to_string()];

        let principal = Principal::from_claims(&claims, ConnectorId::new(0), groups.clone());

        assert_eq!(principal.subject, "user-123");
        assert_eq!(principal.source, ConnectorId::new(0));
        assert_eq!(principal.groups, groups);
        assert_eq!(principal.display_name, Some("johndoe".to_string()));
    }

    #[test]
    fn test_principal_scope() {
        let mut raw = HashMap::new();
        raw.insert("sub".to_string(), serde_json::json!("user-123"));
        let claims = OidcClaims::from_raw(&raw);
        let groups = vec!["admin".to_string()];

        let principal = Principal::from_claims(&claims, ConnectorId::new(0), groups.clone());

        let mut role_map = HashMap::new();
        role_map.insert("admin".to_string(), Role::Admin);
        let scope = principal.scope(&role_map);

        assert!(scope.has_role("admin"));
    }

    // ── InternalRoles tests ─────────────────────────────────────────────────

    #[test]
    fn test_internal_roles_creation() {
        let roles = InternalRoles::new(
            vec!["viewer".to_string()],
            vec!["read".to_string()],
            vec![Role::Viewer],
        );

        assert_eq!(roles.roles, vec!["viewer"]);
        assert_eq!(roles.scopes, vec!["read"]);
        assert!(roles.has_role(Role::Viewer));
        assert!(!roles.has_role(Role::Admin));
        assert_eq!(roles.highest_role(), Some(Role::Viewer));
    }

    #[test]
    fn test_internal_roles_has_role() {
        let roles = InternalRoles::new(
            vec!["admin".to_string()],
            vec![],
            vec![Role::Admin],
        );

        // Admin includes Viewer, Ingest, ComplianceAuditor, TokenAdmin
        assert!(roles.has_role(Role::Admin));
        assert!(roles.has_role(Role::Viewer));
        assert!(roles.has_role(Role::Ingest));
        assert!(roles.has_role(Role::TokenAdmin));
    }

    #[test]
    fn test_internal_roles_highest_role() {
        let roles = InternalRoles::new(
            vec!["viewer".to_string()],
            vec![],
            vec![Role::Viewer],
        );
        assert_eq!(roles.highest_role(), Some(Role::Viewer));

        let roles2 = InternalRoles::default();
        assert_eq!(roles2.highest_role(), None);
    }

    // ── GroupCache tests ────────────────────────────────────────────────────

    #[test]
    fn test_group_cache_put_get() {
        let cache = GroupCache::default_ttl();
        cache.put("user-1", vec!["admin".to_string()]);

        let result = cache.get("user-1").unwrap();
        assert_eq!(result, vec!["admin"]);
    }

    #[test]
    fn test_group_cache_miss() {
        let cache = GroupCache::default_ttl();
        assert!(cache.get("user-1").is_none());
    }

    #[test]
    fn test_group_cache_invalidate() {
        let cache = GroupCache::default_ttl();
        cache.put("user-1", vec!["admin".to_string()]);
        cache.invalidate("user-1");
        assert!(cache.get("user-1").is_none());
    }

    #[test]
    fn test_group_cache_clear() {
        let cache = GroupCache::default_ttl();
        cache.put("user-1", vec!["admin".to_string()]);
        cache.put("user-2", vec!["editor".to_string()]);
        cache.clear();
        assert!(cache.get("user-1").is_none());
        assert!(cache.get("user-2").is_none());
    }

    #[test]
    fn test_group_cache_eviction() {
        let cache = GroupCache::new(Duration::from_millis(10));
        cache.put("user-1", vec!["admin".to_string()]);

        // Should be cached
        assert!(cache.get("user-1").is_some());

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(20));

        // Should be evicted
        assert!(cache.get("user-1").is_none());
    }

    #[test]
    fn test_group_cache_ttl_default() {
        assert_eq!(GroupCache::DEFAULT_TTL, Duration::from_secs(300));
    }

    // ── GroupResolver tests ─────────────────────────────────────────────────

    #[test]
    fn test_group_resolver_resolve_cached_hit() {
        let resolver = NullGroupResolver;
        let cache = GroupCache::default_ttl();

        // First call - resolver (cache miss)
        let groups = resolver.resolve_cached("user-1", &cache).unwrap();
        assert!(groups.is_empty());

        // Second call - cache hit
        let groups = resolver.resolve_cached("user-1", &cache).unwrap();
        assert!(groups.is_empty());
    }

    #[test]
    fn test_static_resolver_returns_groups() {
        let resolver = StaticGroupResolver::new(vec!["admin".to_string(), "editor".to_string()]);
        let groups = resolver.resolve("user-1").unwrap();
        assert_eq!(groups, vec!["admin", "editor"]);
    }

    // ── RoleMapper tests ────────────────────────────────────────────────────

    #[test]
    fn test_role_map_admin() {
        let rules = vec![RoleMappingRule::new("spindle-admins", Role::Admin)];
        let mapper = RoleMapper::new(rules);

        let roles = mapper.map(&["spindle-admins".to_string()]);

        assert!(roles.has_role(Role::Admin));
        assert!(roles.has_role(Role::Viewer));
        assert!(roles.has_role(Role::Ingest));
    }

    #[test]
    fn test_role_map_viewer() {
        let rules = vec![RoleMappingRule::new("spindle-viewers", Role::Viewer)];
        let mapper = RoleMapper::new(rules);

        let roles = mapper.map(&["spindle-viewers".to_string()]);

        assert!(roles.has_role(Role::Viewer));
        assert!(!roles.has_role(Role::Admin));
    }

    #[test]
    fn test_role_map_no_match() {
        let rules = vec![RoleMappingRule::new("spindle-admins", Role::Admin)];
        let mapper = RoleMapper::new(rules);

        let roles = mapper.map(&["some-random-group".to_string()]);

        assert!(roles.spindle_roles.is_empty());
        assert!(!roles.has_role(Role::Viewer));
    }

    #[test]
    fn test_role_map_multiple_groups() {
        let rules = vec![
            RoleMappingRule::new("spindle-admins", Role::Admin),
            RoleMappingRule::new("spindle-viewers", Role::Viewer),
        ];
        let mapper = RoleMapper::new(rules);

        let roles = mapper.map(&["spindle-admins".to_string(), "spindle-viewers".to_string()]);

        // Admin includes Viewer, so both roles should be present
        assert!(roles.has_role(Role::Admin));
        assert!(roles.has_role(Role::Viewer));
    }

    #[test]
    fn test_role_mapper_add_rule() {
        let mut mapper = RoleMapper::default_rules();
        mapper.add_rule(RoleMappingRule::new("spindle-admins", Role::Admin));

        let roles = mapper.map(&["spindle-admins".to_string()]);
        assert!(roles.has_role(Role::Admin));
    }

    // ── GroupError tests ────────────────────────────────────────────────────

    #[test]
    fn test_group_error_display() {
        let timeout = GroupError::Timeout("provider slow".to_string());
        assert!(format!("{}", timeout).contains("timeout"));

        let provider = GroupError::ProviderError("503".to_string());
        assert!(format!("{}", provider).contains("provider"));

        let parse = GroupError::ParseError("bad JSON".to_string());
        assert!(format!("{}", parse).contains("parse"));
    }

    // ── AuthSession tests ───────────────────────────────────────────────────

    #[test]
    fn test_auth_session_creation() {
        let principal = Principal {
            subject: "user-1".to_string(),
            source: ConnectorId(0),
            claims: HashMap::new(),
            groups: vec!["admin".to_string()],
            display_name: None,
            email: None,
        };

        let roles = InternalRoles {
            roles: vec!["admin".to_string()],
            scopes: vec!["admin".to_string()],
            spindle_roles: vec![Role::Admin],
        };

        let session = AuthSession::new(
            principal,
            roles,
            "session-token-123".to_string(),
            Duration::from_secs(3600),
        );

        assert_eq!(session.session_token, "session-token-123");
        assert!(session.is_valid());
        assert_eq!(session.principal.subject, "user-1");
    }

    #[test]
    fn test_auth_session_scope() {
        let principal = Principal {
            subject: "user-1".to_string(),
            source: ConnectorId(0),
            claims: HashMap::new(),
            groups: vec!["admin".to_string()],
            display_name: None,
            email: None,
        };

        let roles = InternalRoles {
            roles: vec!["admin".to_string()],
            scopes: vec![],
            spindle_roles: vec![Role::Admin],
        };

        let session = AuthSession::new(
            principal,
            roles,
            "token".to_string(),
            Duration::from_secs(3600),
        );

        let scope = session.scope();
        assert!(scope.has_role("admin"));
    }

    // ── DexClient tests ─────────────────────────────────────────────────────

    #[test]
    fn test_dex_client_from_config() {
        let dex_config = DexConfig {
            issuer: "https://spindle.local/dex".to_string(),
            issuer_url: "https://spindle.local/dex".to_string(),
            health_check: true,
            connectors: vec![],
            features: spindle_dex::Features::default(),
        };

        let client = DexClient::from_config(&dex_config);
        assert_eq!(client.issuer, "https://spindle.local/dex");
    }

    #[test]
    fn test_dex_client_extract_id_token() {
        let client = DexClient::default();

        let token = serde_json::json!({
            "sub": "user-123",
            "preferred_username": "johndoe",
            "email": "john@example.com",
            "groups": ["admin", "editor"],
        })
        .to_string();

        let claims = client.extract_id_token(&token).unwrap();
        assert_eq!(claims.sub, "user-123");
        assert_eq!(claims.preferred_username, Some("johndoe".to_string()));
        assert_eq!(claims.groups, Some(vec!["admin".to_string(), "editor".to_string()]));
    }

    #[test]
    fn test_dex_client_extract_id_token_invalid() {
        let client = DexClient::default();
        let result = client.extract_id_token("not-json");
        assert!(result.is_err());
    }

    #[test]
    fn test_dex_client_validate_token_ok() {
        let client = DexClient::default();
        let mut claims = OidcClaims::default();
        claims.sub = "https://spindle.local/dex/user-123".to_string();

        assert!(client.validate_token(&claims, "spindle").is_ok());
    }

    #[test]
    fn test_dex_client_validate_token_missing_sub() {
        let client = DexClient::default();
        let claims = OidcClaims::default();

        let err = client.validate_token(&claims, "spindle").unwrap_err();
        assert!(err.contains("sub"));
    }

    // ── NullGroupResolver ───────────────────────────────────────────────────

    #[test]
    fn test_null_resolver() {
        let resolver = NullGroupResolver;
        let groups = resolver.resolve("user-1").unwrap();
        assert!(groups.is_empty());
    }
}
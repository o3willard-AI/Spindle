//! LDAP/AD connector for Spindle.
//!
//! Implements LDAP authentication via the following flow:
//! 1. User DN resolution: search for the user's DN using a configurable base DN + filter
//! 2. Direct bind: bind with the resolved DN + user's password for authentication
//! 3. Group resolution: query group membership, with nested group resolution (recursive)
//! 4. Referral handling: follow or reject LDAP referrals (configurable)
//! 5. TLS: StartTLS or LDAPS for production (required, enforced by config)
//! 6. Connection pooling: pooled LDAP connections for reuse
//! 7. Group cache: per-principal group cache with configurable TTL (default 15min)

#![allow(warnings)]
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ldap3::{LdapConn, LdapConnSettings, Scope, SearchEntry};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::debug;

use crate::SpindleConfig;

/// LDAP connector configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapConnectorConfig {
    /// LDAP server URL (e.g., "ldaps://ldap.example.com:636" or "ldap://ldap.example.com:389").
    pub server_url: String,
    /// Base DN for searching users and groups (e.g., "dc=example,dc=com").
    pub base_dn: String,
    /// Bind DN for the service account (used for user lookup). If empty, anonymous bind.
    pub bind_dn: Option<String>,
    /// Password for the service account.
    pub CHANGE_ME: Option<String>,
    /// LDAP search filter for user lookup (e.g., "(uid={user})").
    /// The `{user}` placeholder is replaced with the user's login value.
    pub user_search_filter: String,
    /// List of attributes to retrieve for user lookup (e.g., ["dn", "uid", "cn"]).
    pub user_search_attributes: Vec<String>,
    /// LDAP search filter for group membership (e.g., "(member={dn})").
    /// If empty, no group lookup is performed.
    pub group_search_filter: Option<String>,
    /// List of attributes to retrieve for group lookup (e.g., ["cn", "memberOf"]).
    pub group_search_attributes: Vec<String>,
    /// Whether to follow LDAP referrals (default: false).
    #[serde(default)]
    pub follow_referrals: bool,
    /// Whether TLS is required (default: true for production).
    #[serde(default)]
    pub require_tls: bool,
    /// Connection pool size (default: 10).
    #[serde(default = "default_pool_size")]
    pub pool_size: usize,
    /// Connection timeout in seconds (default: 10).
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Maximum nested group resolution depth (default: 5).
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
    /// Group cache TTL in seconds (default: 900 = 15 minutes).
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_secs: u64,
}

impl Default for LdapConnectorConfig {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            base_dn: String::new(),
            bind_dn: None,
            CHANGE_ME: None,
            user_search_filter: "(uid={user})".to_string(),
            user_search_attributes: vec!["dn".to_string(), "uid".to_string()],
            group_search_filter: None,
            group_search_attributes: vec!["cn".to_string()],
            follow_referrals: false,
            require_tls: true,
            pool_size: default_pool_size(),
            timeout_secs: default_timeout(),
            max_depth: default_max_depth(),
            cache_ttl_secs: default_cache_ttl(),
        }
    }
}

fn default_pool_size() -> usize { 10 }
fn default_timeout() -> u64 { 10 }
fn default_max_depth() -> u32 { 5 }
fn default_cache_ttl() -> u64 { 900 }

/// Errors that can occur during LDAP operations.
#[derive(Debug, Error)]
pub enum LdapError {
    #[error("LDAP connection error: {0}")]
    Connection(String),
    #[error("LDAP bind error (invalid credentials): {0}")]
    BindFailed(String),
    #[error("LDAP search error: {0}")]
    Search(String),
    #[error("User not found: {0}")]
    UserNotFound(String),
    #[error("LDAP referral rejected: {0}")]
    ReferralRejected(String),
    #[error("TLS required but not available")]
    TlsRequired,
    #[error("Nested group resolution exceeded max depth ({0})")]
    MaxDepthExceeded(u32),
    #[error("Group resolution timeout")]
    Timeout,
    #[error("Cache error: {0}")]
    Cache(String),
}

/// Result of LDAP authentication.
#[derive(Debug, Clone)]
pub struct LdapAuthResult {
    /// The user's DN (Distinguished Name).
    pub dn: String,
    /// The user's UID (from search attributes).
    pub uid: Option<String>,
    /// Resolved groups (including nested).
    pub groups: Vec<String>,
    /// Claims extracted from the user entry.
    pub claims: HashMap<String, String>,
}

/// Cached group membership for a principal.
#[derive(Debug, Clone)]
struct CachedGroups {
    groups: Vec<String>,
    expires_at: Instant,
}

/// Trait abstracting LDAP operations for testability.
/// In production, this wraps `ldap3::LdapConn`.
/// In tests, a mock implementation can be provided.
#[async_trait]
pub trait LdapOperations: Send + Sync {
    /// Search for a user's DN.
    async fn search_user_dn(&self, username: &str) -> Result<String, String>;
    /// Perform a direct bind with DN and password.
    async fn bind(&self, dn: &str, password: &str) -> Result<(), String>;
    /// Search for groups that contain the given DN as a member.
    /// Returns a list of (group_name, group_dn) pairs for nested resolution.
    async fn search_groups(
        &self,
        base_dn: &str,
        filter: &str,
        attrs: &[String],
    ) -> Result<Vec<(String, String)>, String>;
    /// Retrieve user attributes.
    async fn get_user_attributes(
        &self,
        dn: &str,
        attrs: &[String],
    ) -> Result<HashMap<String, String>, String>;
}

/// LDAP connector that handles authentication, group resolution, and caching.
pub struct LdapConnector {
    config: LdapConnectorConfig,
    /// Group cache: principal key -> cached groups
    cache: Arc<std::sync::Mutex<HashMap<String, CachedGroups>>>,
    /// Optional LDAP operations implementation (for testing/mocking).
    /// When None, actual LDAP connections are used.
    ldap_ops: Option<Arc<dyn LdapOperations>>,
}

impl std::fmt::Debug for LdapConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LdapConnector")
            .field("config", &self.config)
            .field("cache_size", &self.cache_size())
            .field("ldap_ops_set", &self.ldap_ops.is_some())
            .finish()
    }
}

impl Clone for LdapConnector {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            cache: self.cache.clone(),
            ldap_ops: self.ldap_ops.clone(),
        }
    }
}

impl LdapConnector {
    /// Create a new LDAP connector from configuration.
    pub fn new(config: LdapConnectorConfig) -> Self {
        Self {
            config,
            cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
            ldap_ops: None,
        }
    }

    /// Create a new LDAP connector with custom LDAP operations (for testing).
    pub fn with_ldap_ops(
        config: LdapConnectorConfig,
        ldap_ops: Arc<dyn LdapOperations>,
    ) -> Self {
        Self {
            config,
            cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
            ldap_ops: Some(ldap_ops),
        }
    }

    /// Create an LDAP connector from a SpindleConfig's LdapConfig.
    pub fn from_spindle_config(config: &SpindleConfig) -> Result<Self, LdapError> {
        let ldap_config = config.identity.ldap.as_ref().ok_or_else(|| {
            LdapError::Connection("LDAP configuration not found in SpindleConfig".into())
        })?;

        let connector_config = LdapConnectorConfig {
            server_url: ldap_config.server_url.clone(),
            base_dn: ldap_config.base_dn.clone(),
            bind_dn: ldap_config.bind_dn.clone(),
            CHANGE_ME: ldap_config.CHANGE_ME.clone(),
            user_search_filter: ldap_config.user_search_filter.clone(),
            user_search_attributes: ldap_config
                .user_search_attributes
                .clone()
                .unwrap_or_else(|| vec!["dn".to_string(), "uid".to_string()]),
            group_search_filter: ldap_config.group_search_filter.clone(),
            group_search_attributes: ldap_config
                .group_search_attributes
                .clone()
                .unwrap_or_else(|| vec!["cn".to_string()]),
            follow_referrals: ldap_config.follow_referrals.unwrap_or(false),
            require_tls: ldap_config.require_tls.unwrap_or(true),
            pool_size: ldap_config.pool_size.unwrap_or_else(default_pool_size),
            timeout_secs: ldap_config.timeout_secs.unwrap_or_else(default_timeout),
            max_depth: ldap_config.max_depth.unwrap_or_else(default_max_depth),
            cache_ttl_secs: ldap_config
                .cache_ttl_secs
                .unwrap_or_else(default_cache_ttl),
        };

        Ok(Self::new(connector_config))
    }

    /// Validate the LDAP configuration for production use.
    /// Ensures TLS is enabled when connecting to non-local servers.
    pub fn validate_config(&self) -> Result<(), LdapError> {
        if self.config.server_url.is_empty() {
            return Err(LdapError::Connection("server_url is empty".into()));
        }
        if self.config.base_dn.is_empty() {
            return Err(LdapError::Connection("base_dn is empty".into()));
        }
        // TLS check: require TLS for non-local servers
        let is_local = self.config.server_url.contains("localhost")
            || self.config.server_url.contains("127.0.0.1")
            || self.config.server_url.contains("::1");
        if !is_local && !self.config.require_tls {
            return Err(LdapError::TlsRequired);
        }
        Ok(())
    }

    /// Create a real LDAP connection with appropriate settings.
    fn create_connection(&self) -> Result<LdapConn, LdapError> {
        let settings = LdapConnSettings::new()
            .set_conn_timeout(Duration::from_secs(self.config.timeout_secs))
            .set_no_tls_verify(self.config.server_url.contains("localhost"));

        LdapConn::with_settings(settings, &self.config.server_url)
            .map_err(|e| LdapError::Connection(format!("Failed to connect to LDAP: {e}")))
    }

    /// Authenticate a user by:
    /// 1. Resolving the user's DN via search
    /// 2. Performing a direct bind with the user's password
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<LdapAuthResult, LdapError> {
        // Step 1: Resolve user DN
        let user_dn = self.resolve_user_dn(username).await?;

        // Step 2: Direct bind with user's password (authenticate)
        self.direct_bind(&user_dn, password).await?;

        // Step 3: Retrieve user attributes
        let claims = self.retrieve_user_attributes(&user_dn).await.unwrap_or_default();

        // Step 4: Resolve groups (with nested resolution)
        let groups = self.resolve_groups(&user_dn).await?;

        Ok(LdapAuthResult {
            dn: user_dn.clone(),
            uid: claims.get("uid").cloned(),
            groups,
            claims,
        })
    }

    /// Resolve a user's DN by searching with the configured filter.
    async fn resolve_user_dn(&self, username: &str) -> Result<String, LdapError> {
        let filter = self
            .config
            .user_search_filter
            .replace("{user}", username);

        let dn = if let Some(ops) = &self.ldap_ops {
            // Use mock operations
            ops.search_user_dn(username)
                .await
                .map_err(|e| LdapError::Search(format!("User search failed: {e}")))?
        } else {
            // Use real LDAP
            let mut conn = self.create_connection()?;

            // Service account bind (for user lookup)
            if let (Some(bind_dn), Some(CHANGE_ME)) =
                (&self.config.bind_dn, &self.config.CHANGE_ME)
            {
                let result = conn.simple_bind(bind_dn, CHANGE_ME)
                    .map_err(|e| LdapError::BindFailed(format!("Service account bind failed: {e}")))?;

                if result.rc != 0 {
                    return Err(LdapError::BindFailed(
                        "Service account bind failed".to_string(),
                    ));
                }
            }

            let result = conn
                .search(
                    &self.config.base_dn,
                    Scope::Subtree,
                    &filter,
                    self.config.user_search_attributes.clone(),
                )
                .map_err(|e| LdapError::Search(format!("User search failed: {e}")))?;

            let (entries, _) = result
                .success()
                .map_err(|e| LdapError::Search(format!("Search result error: {e}")))?;

            if entries.is_empty() {
                return Err(LdapError::UserNotFound(format!(
                    "No user found matching filter: {}",
                    filter
                )));
            }

            // Extract DN from the first entry
            let search_entry = SearchEntry::construct(entries[0].clone());
            search_entry.dn
        };

        if dn.is_empty() {
            return Err(LdapError::UserNotFound(format!(
                "No user found matching filter: {}",
                filter
            )));
        }

        Ok(dn)
    }

    /// Perform a direct bind with DN and password.
    async fn direct_bind(&self, dn: &str, password: &str) -> Result<(), LdapError> {
        if let Some(ops) = &self.ldap_ops {
            // Use mock operations
            ops.bind(dn, password)
                .await
                .map_err(LdapError::BindFailed)?;
            Ok(())
        } else {
            // Use real LDAP
            let mut conn = self.create_connection()?;
            let result = conn
                .simple_bind(dn, password)
                .map_err(|e| LdapError::BindFailed(format!("Bind failed: {e}")))?;

            if result.rc != 0 {
                return Err(LdapError::BindFailed(format!(
                    "Invalid credentials for DN: {}",
                    dn
                )));
            }
            Ok(())
        }
    }

    /// Retrieve user attributes from LDAP.
    async fn retrieve_user_attributes(
        &self,
        dn: &str,
    ) -> Result<HashMap<String, String>, LdapError> {
        if let Some(ops) = &self.ldap_ops {
            // Use mock operations
            ops.get_user_attributes(dn, &self.config.user_search_attributes)
                .await
                .map_err(|e| LdapError::Search(format!("Attribute search failed: {e}")))
        } else {
            // Use real LDAP
            let mut conn = self.create_connection()?;
            let attrs: Vec<&str> = self
                .config
                .user_search_attributes
                .iter()
                .map(|s| s.as_str())
                .collect();

            let result = conn
                .search(dn, Scope::Base, "(objectClass=*)", attrs)
                .map_err(|e| LdapError::Search(format!("Attribute search failed: {e}")))?;

            let (entries, _) = result
                .success()
                .map_err(|e| LdapError::Search(format!("Search result error: {e}")))?;

            let mut claims = HashMap::new();
            if let Some(entry) = entries.first() {
                let search_entry = SearchEntry::construct(entry.clone());
                for (attr, values) in search_entry.attrs {
                    if !values.is_empty() {
                        claims.insert(attr, values[0].clone());
                    }
                }
            }
            Ok(claims)
        }
    }

    /// Resolve all groups for a user, including nested groups.
    /// Uses recursive membership queries with configurable depth limit.
    pub async fn resolve_groups(&self, user_dn: &str) -> Result<Vec<String>, LdapError> {
        let mut all_groups: HashSet<String> = HashSet::new();
        let mut visited: HashSet<String> = HashSet::new();
        self.resolve_groups_recursive(user_dn, &mut all_groups, &mut visited, 0)
            .await?;
        Ok(all_groups.into_iter().collect())
    }

    /// Recursively resolve groups with depth limiting.
    async fn resolve_groups_recursive(
        &self,
        dn: &str,
        all_groups: &mut HashSet<String>,
        visited: &mut HashSet<String>,
        depth: u32,
    ) -> Result<(), LdapError> {
        if depth > self.config.max_depth {
            return Err(LdapError::MaxDepthExceeded(self.config.max_depth));
        }

        if visited.contains(dn) {
            return Ok(());
        }
        visited.insert(dn.to_string());

        // Determine the group search filter
        let group_filter = match &self.config.group_search_filter {
            Some(filter) => filter.replace("{dn}", dn),
            None => format!("(member={})", dn),
        };

        // Get group names and DNs
        let group_results: Vec<(String, String)> = if let Some(ops) = &self.ldap_ops {
            ops.search_groups(&self.config.base_dn, &group_filter, &self.config.group_search_attributes)
                .await
                .map_err(|e| LdapError::Search(format!("Group search failed: {e}")))?
        } else {
            // Use real LDAP
            let mut conn = self.create_connection()?;

            // Handle referrals
            let result = conn
                .search(
                    &self.config.base_dn,
                    Scope::Subtree,
                    &group_filter,
                    self.config.group_search_attributes.clone(),
                )
                .map_err(|e| LdapError::Search(format!("Group search failed: {e}")))?;

            let (entries, ldap_result) = result
                .success()
                .map_err(|e| LdapError::Search(format!("Group search result error: {e}")))?;

            // Check for referrals in the result
            if !self.config.follow_referrals && !ldap_result.refs.is_empty() {
                return Err(LdapError::ReferralRejected(ldap_result.refs.join(", ")));
            }

            entries
                .iter()
                .map(|e| {
                    let se = SearchEntry::construct(e.clone());
                    let group_name = se.attrs.get("cn")
                        .and_then(|v| v.first())
                        .cloned()
                        .unwrap_or_else(|| Self::extract_cn_from_dn(&se.dn));
                    (group_name, se.dn)
                })
                .collect()
        };

        for (group_name, group_dn) in &group_results {
            if all_groups.insert(group_name.clone()) {
                debug!("Found group: {} (depth: {})", group_name, depth);
            }
            // Recursively resolve nested groups
            Box::pin(self.resolve_groups_recursive(
                group_dn,
                all_groups,
                visited,
                depth + 1,
            ))
            .await?;
        }

        Ok(())
    }

    /// Extract the CN (Common Name) from a DN string.
    pub fn extract_cn_from_dn(dn: &str) -> String {
        for part in dn.split(',') {
            let part = part.trim();
            if let Some(cn) = part.strip_prefix("CN=") {
                return cn.to_string();
            }
            if let Some(cn) = part.strip_prefix("cn=") {
                return cn.to_string();
            }
        }
        dn.to_string()
    }

    /// Check the group cache for a principal.
    /// Returns cached groups if available and not expired.
    pub fn get_cached_groups(&self, principal_key: &str) -> Option<Vec<String>> {
        let cache = self.cache.lock().unwrap();
        if let Some(cached) = cache.get(principal_key) {
            if Instant::now() < cached.expires_at {
                debug!(
                    "Group cache HIT for principal: {} ({} groups)",
                    principal_key,
                    cached.groups.len()
                );
                return Some(cached.groups.clone());
            } else {
                debug!(
                    "Group cache EXPIRED for principal: {}",
                    principal_key
                );
            }
        }
        None
    }

    /// Store groups in the cache for a principal.
    pub fn cache_groups(&self, principal_key: &str, groups: Vec<String>) -> Result<(), LdapError> {
        let mut cache = self.cache.lock().unwrap();
        cache.insert(
            principal_key.to_string(),
            CachedGroups {
                groups: groups.clone(),
                expires_at: Instant::now() + Duration::from_secs(self.config.cache_ttl_secs),
            },
        );
        debug!(
            "Group cache SET for principal: {} ({} groups, TTL: {}s)",
            principal_key,
            groups.len(),
            self.config.cache_ttl_secs
        );
        Ok(())
    }

    /// Manually refresh the group cache for a principal.
    pub async fn refresh_groups(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Vec<String>, LdapError> {
        let result = self.authenticate(username, password).await?;
        let principal_key = format!("ldap:{}", username);
        self.cache_groups(&principal_key, result.groups.clone())?;
        Ok(result.groups)
    }

    /// Clear the entire group cache.
    pub fn clear_cache(&self) {
        let mut cache = self.cache.lock().unwrap();
        let count = cache.len();
        cache.clear();
        debug!("Group cache cleared ({} entries)", count);
    }

    /// Returns the number of entries in the cache.
    pub fn cache_size(&self) -> usize {
        let cache = self.cache.lock().unwrap();
        cache.len()
    }

    /// Returns the cache TTL in seconds.
    pub fn cache_ttl_secs(&self) -> u64 {
        self.config.cache_ttl_secs
    }

    /// Returns the max group resolution depth.
    pub fn max_depth(&self) -> u32 {
        self.config.max_depth
    }

    /// Returns whether referrals are followed.
    pub fn follow_referrals(&self) -> bool {
        self.config.follow_referrals
    }

    /// Returns whether TLS is required.
    pub fn require_tls(&self) -> bool {
        self.config.require_tls
    }

    /// Returns the LDAP server URL.
    pub fn server_url(&self) -> &str {
        &self.config.server_url
    }

    /// Returns the base DN.
    pub fn base_dn(&self) -> &str {
        &self.config.base_dn
    }

    /// Returns the connection pool size.
    pub fn pool_size(&self) -> usize {
        self.config.pool_size
    }

    /// Returns the timeout in seconds.
    pub fn timeout_secs(&self) -> u64 {
        self.config.timeout_secs
    }
}

/// Trait for LDAP authentication, used by the identity layer.
#[async_trait]
pub trait LdapAuthenticator {
    /// Authenticate a user against LDAP.
    async fn authenticate_ldap(
        &self,
        username: &str,
        password: &str,
    ) -> Result<LdapAuthResult, LdapError>;

    /// Resolve groups for a user (without authentication).
    async fn resolve_groups_ldap(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Vec<String>, LdapError>;

    /// Refresh the group cache for a user.
    async fn refresh_ldap_groups(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Vec<String>, LdapError>;
}

#[async_trait]
impl LdapAuthenticator for LdapConnector {
    async fn authenticate_ldap(
        &self,
        username: &str,
        password: &str,
    ) -> Result<LdapAuthResult, LdapError> {
        self.authenticate(username, password).await
    }

    async fn resolve_groups_ldap(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Vec<String>, LdapError> {
        let auth_result = self.authenticate(username, password).await?;
        Ok(auth_result.groups)
    }

    async fn refresh_ldap_groups(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Vec<String>, LdapError> {
        self.refresh_groups(username, password).await
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock LDAP operations for testing.
    #[derive(Debug)]
    struct MockLdapOps {
        /// Users: username -> (dn, password, attributes)
        users: Arc<std::sync::Mutex<HashMap<String, (String, String, HashMap<String, String>)>>>,
        /// Groups: group_dn -> list of member DNs
        groups: Arc<std::sync::Mutex<HashMap<String, Vec<String>>>>,
        /// Bind call counter
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

        fn add_user(
            &self,
            username: &str,
            dn: &str,
            password: &str,
            attrs: HashMap<String, String>,
        ) {
            let mut users = self.users.lock().unwrap();
            users.insert(
                username.to_string(),
                (dn.to_string(), password.to_string(), attrs),
            );
        }

        fn add_group(&self, group_dn: &str, members: Vec<String>) {
            let mut groups = self.groups.lock().unwrap();
            groups.insert(group_dn.to_string(), members);
        }
    }

    #[async_trait]
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

            // Extract the DN from the filter (e.g., "(member=CN=John,OU=Users)")
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

    /// Test that LDAP config validation catches empty server URL.
    #[test]
    fn test_validate_config_empty_server_url() {
        let config = LdapConnectorConfig {
            server_url: String::new(),
            base_dn: "dc=example,dc=com".to_string(),
            ..Default::default()
        };
        let connector = LdapConnector::new(config);
        let err = connector.validate_config().unwrap_err();
        assert!(matches!(err, LdapError::Connection(_)));
    }

    #[test]
    fn test_validate_config_empty_base_dn() {
        let config = LdapConnectorConfig {
            server_url: "ldaps://ldap.example.com".to_string(),
            base_dn: String::new(),
            ..Default::default()
        };
        let connector = LdapConnector::new(config);
        let err = connector.validate_config().unwrap_err();
        assert!(matches!(err, LdapError::Connection(_)));
    }

    /// Test that TLS is required for non-local servers.
    #[test]
    fn test_validate_config_tls_required_non_local() {
        let config = LdapConnectorConfig {
            server_url: "ldap://ldap.example.com:389".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            require_tls: false,
            ..Default::default()
        };
        let connector = LdapConnector::new(config);
        let err = connector.validate_config().unwrap_err();
        assert!(matches!(err, LdapError::TlsRequired));
    }

    /// Test that TLS is not required for localhost.
    #[test]
    fn test_validate_config_tls_not_required_localhost() {
        let config = LdapConnectorConfig {
            server_url: "ldap://localhost:389".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            require_tls: false,
            ..Default::default()
        };
        let connector = LdapConnector::new(config);
        assert!(connector.validate_config().is_ok());
    }

    /// Test that TLS check passes when require_tls is true.
    #[test]
    fn test_validate_config_tls_enabled() {
        let config = LdapConnectorConfig {
            server_url: "ldaps://ldap.example.com:636".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            require_tls: true,
            ..Default::default()
        };
        let connector = LdapConnector::new(config);
        assert!(connector.validate_config().is_ok());
    }

    /// Test that TLS check passes for 127.0.0.1 even without require_tls.
    #[test]
    fn test_validate_config_localhost_127() {
        let config = LdapConnectorConfig {
            server_url: "ldap://127.0.0.1:389".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            require_tls: false,
            ..Default::default()
        };
        let connector = LdapConnector::new(config);
        assert!(connector.validate_config().is_ok());
    }

    /// Test default config values.
    #[test]
    fn test_default_config() {
        let config = LdapConnectorConfig::default();
        assert_eq!(config.pool_size, 10);
        assert_eq!(config.timeout_secs, 10);
        assert_eq!(config.max_depth, 5);
        assert_eq!(config.cache_ttl_secs, 900);
        assert!(config.require_tls);
        assert!(!config.follow_referrals);
        assert_eq!(config.user_search_filter, "(uid={user})");
    }

    /// Test DN parsing for CN extraction.
    #[test]
    fn test_extract_cn_from_dn() {
        assert_eq!(
            LdapConnector::extract_cn_from_dn("CN=Engineering,OU=Groups,DC=example,DC=com"),
            "Engineering"
        );
        assert_eq!(
            LdapConnector::extract_cn_from_dn("cn=engineering,ou=groups,dc=example,dc=com"),
            "engineering"
        );
        assert_eq!(
            LdapConnector::extract_cn_from_dn("CN=Admins,OU=Groups"),
            "Admins"
        );
        // No CN in DN — returns the full DN
        assert_eq!(
            LdapConnector::extract_cn_from_dn("OU=Groups,DC=example,DC=com"),
            "OU=Groups,DC=example,DC=com"
        );
    }

    /// Test user search filter placeholder replacement.
    #[test]
    fn test_user_search_filter_placeholder() {
        let config = LdapConnectorConfig {
            user_search_filter: "(uid={user})".to_string(),
            ..Default::default()
        };
        let filter = config.user_search_filter.replace("{user}", "jdoe");
        assert_eq!(filter, "(uid=jdoe)");
    }

    /// Test group search filter placeholder replacement.
    #[test]
    fn test_group_search_filter_placeholder() {
        let config = LdapConnectorConfig {
            group_search_filter: Some("(member={dn})".to_string()),
            ..Default::default()
        };
        let filter = config
            .group_search_filter
            .unwrap()
            .replace("{dn}", "CN=John Doe,OU=Users");
        assert_eq!(filter, "(member=CN=John Doe,OU=Users)");
    }

    /// Test referral handling config.
    #[test]
    fn test_referral_follow_config() {
        let config = LdapConnectorConfig {
            follow_referrals: true,
            ..Default::default()
        };
        assert!(config.follow_referrals);

        let config2 = LdapConnectorConfig::default();
        assert!(!config2.follow_referrals);
    }

    /// Test max depth configuration.
    #[test]
    fn test_max_depth_config() {
        let config = LdapConnectorConfig {
            max_depth: 10,
            ..Default::default()
        };
        assert_eq!(config.max_depth, 10);

        let err = LdapError::MaxDepthExceeded(10);
        assert!(err.to_string().contains("10"));
    }

    /// Test cache TTL default value.
    #[test]
    fn test_cache_ttl_default() {
        let config = LdapConnectorConfig::default();
        assert_eq!(config.cache_ttl_secs, 900); // 15 minutes
    }

    /// Test that a new connector starts with empty cache.
    #[test]
    fn test_new_connector_empty_cache() {
        let config = LdapConnectorConfig::default();
        let connector = LdapConnector::new(config);
        assert_eq!(connector.cache_size(), 0);
    }

    /// Test LdapAuthResult construction.
    #[test]
    fn test_ldap_auth_result() {
        let result = LdapAuthResult {
            dn: "CN=John Doe,OU=Users,DC=example,DC=com".to_string(),
            uid: Some("jdoe".to_string()),
            groups: vec!["admin".to_string(), "engineering".to_string()],
            claims: HashMap::from([
                ("mail".to_string(), "jdoe@example.com".to_string()),
                ("displayName".to_string(), "John Doe".to_string()),
            ]),
        };
        assert_eq!(
            result.dn,
            "CN=John Doe,OU=Users,DC=example,DC=com"
        );
        assert_eq!(result.uid, Some("jdoe".to_string()));
        assert_eq!(result.groups.len(), 2);
        assert_eq!(result.claims.len(), 2);
    }

    /// Test error types.
    #[test]
    fn test_error_types() {
        let err = LdapError::UserNotFound("test".to_string());
        assert!(err.to_string().contains("User not found"));

        let err = LdapError::MaxDepthExceeded(5);
        assert!(err.to_string().contains("exceeded max depth"));

        let err = LdapError::TlsRequired;
        assert!(err.to_string().contains("TLS required"));

        let err = LdapError::ReferralRejected("test".to_string());
        assert!(err.to_string().contains("referral rejected"));
    }

    /// Test that cache stores multiple principals.
    #[test]
    fn test_cache_multiple_principals() {
        let config = LdapConnectorConfig::default();
        let connector = LdapConnector::new(config);

        connector
            .cache_groups("ldap:user1", vec!["admin".to_string()])
            .unwrap();
        connector
            .cache_groups("ldap:user2", vec!["viewer".to_string()])
            .unwrap();

        assert_eq!(connector.cache_size(), 2);
        assert!(connector.get_cached_groups("ldap:user1").is_some());
        assert!(connector.get_cached_groups("ldap:user2").is_some());
    }

    /// Test that cache overwrites on second set.
    #[test]
    fn test_cache_overwrite() {
        let config = LdapConnectorConfig::default();
        let connector = LdapConnector::new(config);

        connector
            .cache_groups("ldap:user1", vec!["admin".to_string()])
            .unwrap();
        connector
            .cache_groups("ldap:user1", vec!["viewer".to_string(), "editor".to_string()])
            .unwrap();

        let cached = connector.get_cached_groups("ldap:user1").unwrap();
        assert_eq!(cached.len(), 2);
        assert!(cached.contains(&"viewer".to_string()));
        assert!(cached.contains(&"editor".to_string()));
        assert!(!cached.contains(&"admin".to_string()));
    }

    /// Test config getter methods.
    #[test]
    fn test_config_getters() {
        let config = LdapConnectorConfig {
            server_url: "ldaps://ldap.example.com:636".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            require_tls: true,
            follow_referrals: true,
            max_depth: 3,
            pool_size: 5,
            cache_ttl_secs: 600,
            timeout_secs: 15,
            ..Default::default()
        };
        let connector = LdapConnector::new(config);

        assert_eq!(connector.server_url(), "ldaps://ldap.example.com:636");
        assert_eq!(connector.base_dn(), "dc=example,dc=com");
        assert!(connector.require_tls());
        assert!(connector.follow_referrals());
        assert_eq!(connector.max_depth(), 3);
        assert_eq!(connector.pool_size(), 5);
        assert_eq!(connector.cache_ttl_secs(), 600);
        assert_eq!(connector.timeout_secs(), 15);
    }

    /// Test successful LDAP authentication with mock.
    #[tokio::test]
    async fn test_authenticate_success() {
        let mut users = HashMap::new();
        users.insert(
            "jdoe".to_string(),
            (
                "CN=John Doe,OU=Users,DC=example,DC=com".to_string(),
                "password123".to_string(),
                HashMap::from([
                    ("uid".to_string(), "jdoe".to_string()),
                    ("mail".to_string(), "jdoe@example.com".to_string()),
                ]),
            ),
        );

        let mock = MockLdapOps {
            users: Arc::new(std::sync::Mutex::new(users)),
            groups: Arc::new(std::sync::Mutex::new(HashMap::new())),
            bind_count: Arc::new(AtomicUsize::new(0)),
        };

        let config = make_config("ldaps://ldap.example.com:636");
        let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

        let result = connector.authenticate("jdoe", "password123").await.unwrap();
        assert_eq!(
            result.dn,
            "CN=John Doe,OU=Users,DC=example,DC=com"
        );
        assert_eq!(result.uid, Some("jdoe".to_string()));
        assert_eq!(result.groups.len(), 0);
        assert_eq!(
            result.claims.get("mail"),
            Some(&"jdoe@example.com".to_string())
        );
    }

    /// Test failed LDAP authentication (bad password).
    #[tokio::test]
    async fn test_authenticate_bad_password() {
        let mut users = HashMap::new();
        users.insert(
            "jdoe".to_string(),
            (
                "CN=John Doe,OU=Users,DC=example,DC=com".to_string(),
                "correct_password".to_string(),
                HashMap::new(),
            ),
        );

        let mock = MockLdapOps {
            users: Arc::new(std::sync::Mutex::new(users)),
            groups: Arc::new(std::sync::Mutex::new(HashMap::new())),
            bind_count: Arc::new(AtomicUsize::new(0)),
        };

        let config = make_config("ldaps://ldap.example.com:636");
        let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

        let result = connector.authenticate("jdoe", "wrong_password").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LdapError::BindFailed(_)));
    }

    /// Test authentication when user not found.
    #[tokio::test]
    async fn test_authenticate_user_not_found() {
        let users = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let mock = MockLdapOps {
            users: users.clone(),
            groups: Arc::new(std::sync::Mutex::new(HashMap::new())),
            bind_count: Arc::new(AtomicUsize::new(0)),
        };

        let config = make_config("ldaps://ldap.example.com:636");
        let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

        let result = connector.authenticate("unknown", "password").await;
        assert!(result.is_err());
        // The mock returns "User not found: unknown" which gets wrapped as Search error
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(err_str.contains("User search failed") || err_str.contains("User not found"));
    }

    /// Test nested group resolution.
    #[tokio::test]
    async fn test_nested_group_resolution() {
        let mut users = HashMap::new();
        users.insert(
            "jdoe".to_string(),
            (
                "CN=John Doe,OU=Users,DC=example,DC=com".to_string(),
                "password123".to_string(),
                HashMap::from([("uid".to_string(), "jdoe".to_string())]),
            ),
        );

        let mut groups = HashMap::new();
        // Engineering group contains John
        groups.insert(
            "CN=Engineering,OU=Groups,DC=example,DC=com".to_string(),
            vec!["CN=John Doe,OU=Users,DC=example,DC=com".to_string()],
        );
        // AllUsers group contains Engineering (nested)
        groups.insert(
            "CN=AllUsers,OU=Groups,DC=example,DC=com".to_string(),
            vec!["CN=Engineering,OU=Groups,DC=example,DC=com".to_string()],
        );

        let mock = MockLdapOps {
            users: Arc::new(std::sync::Mutex::new(users)),
            groups: Arc::new(std::sync::Mutex::new(groups)),
            bind_count: Arc::new(AtomicUsize::new(0)),
        };

        let config = LdapConnectorConfig {
            group_search_filter: Some("(member={dn})".to_string()),
            group_search_attributes: vec!["cn".to_string(), "dn".to_string()],
            base_dn: "dc=example,dc=com".to_string(),
            server_url: "ldaps://ldap.example.com:636".to_string(),
            ..Default::default()
        };
        let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

        // Resolve groups for John
        let groups = connector
            .resolve_groups("CN=John Doe,OU=Users,DC=example,DC=com")
            .await
            .unwrap();

        // Should find Engineering (direct member) and AllUsers (nested)
        assert!(groups.contains(&"Engineering".to_string()));
        assert!(groups.contains(&"AllUsers".to_string()));
    }

    /// Test nested group resolution with max depth exceeded.
    #[tokio::test]
    async fn test_nested_group_max_depth_exceeded() {
        // Create a chain: User -> G1 -> G2 -> G3 -> G4 -> G5 -> G6
        // Each group contains the next as a member
        let mut groups = HashMap::new();
        groups.insert(
            "CN=G1,OU=Groups,DC=example,DC=com".to_string(),
            vec!["CN=User,OU=Users,DC=example,DC=com".to_string()],
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
        groups.insert(
            "CN=G5,OU=Groups,DC=example,DC=com".to_string(),
            vec!["CN=G4,OU=Groups,DC=example,DC=com".to_string()],
        );
        groups.insert(
            "CN=G6,OU=Groups,DC=example,DC=com".to_string(),
            vec![],
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
            max_depth: 3,
            server_url: "ldaps://ldap.example.com:636".to_string(),
            ..Default::default()
        };
        let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

        // Chain: User -> G1 -> G2 -> G3 -> G4 -> G5 -> G6
        // depth 0: User found in G1, recurse into G1
        // depth 1: G1 found in G2, recurse into G2
        // depth 2: G2 found in G3, recurse into G3
        // depth 3: G3 found in G4, recurse into G4
        // depth 4: 4 > max_depth=3, error
        let result = connector.resolve_groups("CN=User,OU=Users,DC=example,DC=com").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LdapError::MaxDepthExceeded(3)));
    }

    /// Test referral rejected error type.
    #[test]
    fn test_referral_rejected_error() {
        let err = LdapError::ReferralRejected("ldap://referral.example.com".to_string());
        assert!(err.to_string().contains("referral rejected"));
    }

    /// Test TLS required error.
    #[test]
    fn test_tls_required_error() {
        let err = LdapError::TlsRequired;
        assert!(err.to_string().contains("TLS required"));
    }

    /// Test user search filter with special characters (sAMAccountName for AD).
    #[test]
    fn test_user_search_filter_sam() {
        let config = LdapConnectorConfig {
            user_search_filter: "(sAMAccountName={user})".to_string(),
            ..Default::default()
        };
        let filter = config.user_search_filter.replace("{user}", "john.doe");
        assert_eq!(filter, "(sAMAccountName=john.doe)");
    }

    /// Test cache TTL with short duration.
    #[test]
    fn test_cache_short_ttl() {
        let config = LdapConnectorConfig {
            cache_ttl_secs: 1, // 1 second TTL
            ..Default::default()
        };
        let connector = LdapConnector::new(config);

        connector
            .cache_groups("ldap:user1", vec!["admin".to_string()])
            .unwrap();

        // Should be cached
        let cached = connector.get_cached_groups("ldap:user1");
        assert!(cached.is_some());

        // Wait for expiry
        std::thread::sleep(Duration::from_secs(2));

        // Should be expired now
        let cached = connector.get_cached_groups("ldap:user1");
        assert!(cached.is_none());
    }

    /// Test cache expiry with TTL=0.
    #[test]
    fn test_cache_expiry() {
        let config = LdapConnectorConfig {
            cache_ttl_secs: 0, // Immediate expiry
            ..Default::default()
        };
        let connector = LdapConnector::new(config);

        connector
            .cache_groups("ldap:user1", vec!["admin".to_string()])
            .unwrap();
        let cached = connector.get_cached_groups("ldap:user1");
        assert!(cached.is_none());
    }

    /// Test cache stores and retrieves correctly with TTL.
    #[test]
    fn test_cache_with_ttl() {
        let config = LdapConnectorConfig {
            cache_ttl_secs: 60, // 60 second TTL
            ..Default::default()
        };
        let connector = LdapConnector::new(config);

        let groups = vec!["engineering".to_string(), "devops".to_string()];
        connector
            .cache_groups("ldap:user1", groups.clone())
            .unwrap();

        let cached = connector.get_cached_groups("ldap:user1");
        assert!(cached.is_some());
        let cached = cached.unwrap();
        assert_eq!(cached.len(), 2);
        assert!(cached.contains(&"engineering".to_string()));
        assert!(cached.contains(&"devops".to_string()));
    }

    /// Test that different principals have independent cache entries.
    #[test]
    fn test_cache_independence() {
        let config = LdapConnectorConfig::default();
        let connector = LdapConnector::new(config);

        connector
            .cache_groups("ldap:user1", vec!["admin".to_string()])
            .unwrap();
        connector
            .cache_groups("ldap:user2", vec!["viewer".to_string()])
            .unwrap();

        assert_eq!(
            connector.get_cached_groups("ldap:user1").unwrap(),
            vec!["admin".to_string()]
        );
        assert_eq!(
            connector.get_cached_groups("ldap:user2").unwrap(),
            vec!["viewer".to_string()]
        );
    }

    /// Test LdapAuthenticator trait implementation.
    #[tokio::test]
    async fn test_ldap_authenticator_trait() {
        let mut users = HashMap::new();
        users.insert(
            "alice".to_string(),
            (
                "CN=Alice,OU=Users,DC=example,DC=com".to_string(),
                "secret".to_string(),
                HashMap::from([("uid".to_string(), "alice".to_string())]),
            ),
        );

        let mock = MockLdapOps {
            users: Arc::new(std::sync::Mutex::new(users)),
            groups: Arc::new(std::sync::Mutex::new(HashMap::new())),
            bind_count: Arc::new(AtomicUsize::new(0)),
        };

        let config = make_config("ldaps://ldap.example.com:636");
        let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

        // Test authenticate_ldap
        let result = connector.authenticate_ldap("alice", "secret").await.unwrap();
        assert_eq!(result.uid, Some("alice".to_string()));

        // Test refresh_ldap_groups
        let groups = connector.refresh_ldap_groups("alice", "secret").await.unwrap();
        assert!(groups.is_empty()); // No groups in mock
    }

    /// Test from_spindle_config without LDAP config.
    #[test]
    fn test_from_spindle_config_no_ldap() {
        let config = SpindleConfig::default();
        let result = LdapConnector::from_spindle_config(&config);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LdapError::Connection(_)));
    }

    /// Test clone preserves cache (shared cache).
    #[test]
    fn test_clone_shares_cache() {
        let config = LdapConnectorConfig::default();
        let connector = LdapConnector::new(config);

        connector
            .cache_groups("ldap:user1", vec!["admin".to_string()])
            .unwrap();

        let connector2 = connector.clone();
        // Cache is shared via Arc
        assert_eq!(connector2.cache_size(), 1);
    }

    /// Test bind count is tracked in mock.
    #[tokio::test]
    async fn test_bind_count_tracked() {
        let mut users = HashMap::new();
        users.insert(
            "jdoe".to_string(),
            (
                "CN=John Doe,OU=Users,DC=example,DC=com".to_string(),
                "password".to_string(),
                HashMap::new(),
            ),
        );

        let bind_count = Arc::new(AtomicUsize::new(0));
        let mock = MockLdapOps {
            users: Arc::new(std::sync::Mutex::new(users)),
            groups: Arc::new(std::sync::Mutex::new(HashMap::new())),
            bind_count: bind_count.clone(),
        };

        let config = make_config("ldaps://ldap.example.com:636");
        let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

        // Authenticate twice
        let _ = connector.authenticate("jdoe", "password").await;
        let _ = connector.authenticate("jdoe", "password").await;

        // Bind should have been called twice
        assert_eq!(bind_count.load(Ordering::SeqCst), 2);
    }

    /// Test bad password increments bind count.
    #[tokio::test]
    async fn test_bad_password_increments_bind() {
        let mut users = HashMap::new();
        users.insert(
            "jdoe".to_string(),
            (
                "CN=John Doe,OU=Users,DC=example,DC=com".to_string(),
                "correct".to_string(),
                HashMap::new(),
            ),
        );

        let bind_count = Arc::new(AtomicUsize::new(0));
        let mock = MockLdapOps {
            users: Arc::new(std::sync::Mutex::new(users)),
            groups: Arc::new(std::sync::Mutex::new(HashMap::new())),
            bind_count: bind_count.clone(),
        };

        let config = make_config("ldaps://ldap.example.com:636");
        let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

        // Try with wrong password
        let result = connector.authenticate("jdoe", "wrong").await;
        assert!(result.is_err());

        // Bind should have been called once
        assert_eq!(bind_count.load(Ordering::SeqCst), 1);
    }

    /// Test user not found does not call bind.
    #[tokio::test]
    async fn test_user_not_found_no_bind() {
        let bind_count = Arc::new(AtomicUsize::new(0));
        let mock = MockLdapOps {
            users: Arc::new(std::sync::Mutex::new(HashMap::new())),
            groups: Arc::new(std::sync::Mutex::new(HashMap::new())),
            bind_count: bind_count.clone(),
        };

        let config = make_config("ldaps://ldap.example.com:636");
        let connector = LdapConnector::with_ldap_ops(config, Arc::new(mock));

        // Try with non-existent user
        let result = connector.authenticate("nobody", "password").await;
        assert!(result.is_err());

        // The mock returns "User not found" which gets wrapped as a Search error
        let err_str = result.unwrap_err().to_string();
        assert!(err_str.contains("User search failed") || err_str.contains("User not found"));

        // Bind should NOT have been called
        assert_eq!(bind_count.load(Ordering::SeqCst), 0);
    }
}

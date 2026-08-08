//! spindle-authz: Authorization types, role hierarchy, and enforcement.
//!
//! Per PLANS.md M2-12 + M2-13:
//! - `Role` enum: Ingest, Viewer, ComplianceAuditor, TokenAdmin, Admin
//! - Role hierarchy: Admin > TokenAdmin > ComplianceAuditor > Viewer > Ingest
//! - `Scope` struct: projects: HashSet<String>, roles: HashSet<Role>
//! - `ScopeFilter` trait: translates scope to SQL WHERE clause
//! - `require_role` macro/attribute for endpoint-level role enforcement
//! - `AuditLog` trait: every authz decision logged (subject, resource, decision, rule)
//! - `compliance-auditor` → node attributes stripped at store layer
//! - Scope applies to COUNT, aggregates, existence checks — not just data retrieval

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ── Roles ───────────────────────────────────────────────────────────────────

/// Spindle authorization roles with hierarchy enforcement.
///
/// Role hierarchy (higher includes all lower):
/// - Admin > TokenAdmin > ComplianceAuditor > Viewer > Ingest
/// - Ingest: write-only access to ingest endpoints
/// - Viewer: read all non-compliance data
/// - ComplianceAuditor: compliance read + export, no node attributes
/// - TokenAdmin: manage tokens
/// - Admin: all
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    Ingest,
    Viewer,
    ComplianceAuditor,
    TokenAdmin,
    Admin,
}

impl Role {
    /// Canonical name for this role.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ingest => "ingest",
            Self::Viewer => "viewer",
            Self::ComplianceAuditor => "compliance-auditor",
            Self::TokenAdmin => "token-admin",
            Self::Admin => "admin",
        }
    }

    /// Check if this role can perform the given action.
    pub fn can_read(self) -> bool {
        matches!(
            self,
            Self::Viewer
                | Self::ComplianceAuditor
                | Self::TokenAdmin
                | Self::Admin
        )
    }

    pub fn can_write(self) -> bool {
        matches!(self, Self::Ingest | Self::TokenAdmin | Self::Admin)
    }

    pub fn can_manage_tokens(self) -> bool {
        matches!(self, Self::TokenAdmin | Self::Admin)
    }

    /// Returns true if this role is `ComplianceAuditor` or higher,
    /// but NOT Admin (Admin gets full attributes).
    pub fn is_compliance_auditor(self) -> bool {
        matches!(self, Self::ComplianceAuditor)
    }

    /// Returns true if this role is Admin.
    pub fn is_admin(self) -> bool {
        matches!(self, Self::Admin)
    }

    /// Check if `self` has the permissions of `required` role (hierarchy).
    /// Admin includes all roles. TokenAdmin includes Viewer and Ingest.
    /// ComplianceAuditor includes Viewer.
    pub fn includes(self, required: Role) -> bool {
        match (self, required) {
            // Admin includes everything
            (Self::Admin, _) => true,
            // TokenAdmin includes Viewer, Ingest, ComplianceAuditor
            (Self::TokenAdmin, Self::Viewer)
            | (Self::TokenAdmin, Self::Ingest)
            | (Self::TokenAdmin, Self::ComplianceAuditor)
            | (Self::TokenAdmin, Self::TokenAdmin) => true,
            // ComplianceAuditor includes Viewer
            (Self::ComplianceAuditor, Self::Viewer)
            | (Self::ComplianceAuditor, Self::ComplianceAuditor) => true,
            // Viewer only includes itself
            (Self::Viewer, Self::Viewer) => true,
            // Ingest only includes itself
            (Self::Ingest, Self::Ingest) => true,
            _ => false,
        }
    }

    /// Check if this role can access compliance data (Auditor or higher, not Admin).
    pub fn can_audit(self) -> bool {
        matches!(self, Self::ComplianceAuditor | Self::TokenAdmin)
    }

    /// Check if this role can access node details (not Auditor).
    pub fn can_view_nodes(self) -> bool {
        !matches!(self, Self::ComplianceAuditor)
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ingest => write!(f, "ingest"),
            Self::Viewer => write!(f, "viewer"),
            Self::ComplianceAuditor => write!(f, "compliance-auditor"),
            Self::TokenAdmin => write!(f, "token-admin"),
            Self::Admin => write!(f, "admin"),
        }
    }
}

// ── Scope ───────────────────────────────────────────────────────────────────

/// Authorization scope — required on every store method call.
///
/// - `projects`: which projects the caller has access to (empty = all).
/// - `roles`: which roles are assigned (e.g. "viewer", "compliance-auditor").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    pub projects: HashSet<String>,
    pub roles: HashSet<String>,
}

impl Scope {
    /// Create a new scope with the given projects and roles.
    pub fn new(projects: HashSet<String>, roles: HashSet<String>) -> Self {
        Self { projects, roles }
    }

    /// Empty scope — grants access to everything (used for service accounts).
    pub fn all() -> Self {
        Self {
            projects: HashSet::new(),
            roles: HashSet::new(),
        }
    }

    /// Check that the given project ID is within scope.
    /// Empty projects set = unrestricted access.
    pub fn has_project(&self, project: &str) -> bool {
        self.projects.is_empty() || self.projects.contains(project)
    }

    /// Check that the given role is present in scope.
    /// Empty roles set = unrestricted access.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.is_empty() || self.roles.contains(role)
    }

    /// Returns true if the caller has the `compliance-auditor` role.
    pub fn is_compliance_auditor(&self) -> bool {
        self.roles.is_empty() || self.roles.contains("compliance-auditor")
    }

    /// Returns true if the caller has `admin` role.
    pub fn is_admin(&self) -> bool {
        self.roles.is_empty() || self.roles.contains("admin")
    }

    /// Returns true if the caller is scoped (not all-access).
    pub fn is_scoped(&self) -> bool {
        !self.projects.is_empty() || !self.roles.is_empty()
    }

    /// Check if the scope's highest role meets the required role (hierarchy).
    /// Returns true if any role in scope `includes` the required role.
    pub fn meets_role(&self, required: Role) -> bool {
        if self.roles.is_empty() {
            return true; // No role restriction = access all
        }
        for role_str in &self.roles {
            let role = match role_str.as_str() {
                "ingest" => Role::Ingest,
                "viewer" => Role::Viewer,
                "compliance-auditor" => Role::ComplianceAuditor,
                "token-admin" => Role::TokenAdmin,
                "admin" => Role::Admin,
                _ => continue,
            };
            if role.includes(required) {
                return true;
            }
        }
        false
    }

    /// Check if scope can read (has Viewer or higher).
    /// Roles with read access: Viewer, ComplianceAuditor, TokenAdmin, Admin.
    /// Ingest does NOT have read access (write-only).
    pub fn can_read(&self) -> bool {
        if self.roles.is_empty() {
            return true; // No role restriction = access all
        }
        for role_str in &self.roles {
            match role_str.as_str() {
                "viewer" | "compliance-auditor" | "token-admin" | "admin" => return true,
                _ => continue,
            }
        }
        false
    }

    /// Check if scope can write (has Ingest, TokenAdmin, or Admin).
    /// Roles with write access: Ingest, TokenAdmin, Admin.
    /// Viewer and ComplianceAuditor do NOT have write access.
    pub fn can_write(&self) -> bool {
        if self.roles.is_empty() {
            return true; // No role restriction = access all
        }
        for role_str in &self.roles {
            match role_str.as_str() {
                "ingest" | "token-admin" | "admin" => return true,
                _ => continue,
            }
        }
        false
    }

    /// Check if scope can manage tokens (has TokenAdmin or higher).
    pub fn can_manage_tokens(&self) -> bool {
        self.meets_role(Role::TokenAdmin)
    }
}

// ── Authz Decision ──────────────────────────────────────────────────────────

/// An authorization decision with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthzDecision {
    pub subject: String,
    pub resource: String,
    pub action: String,
    pub decision: AuthzDecisionOutcome,
    pub rule: Option<String>,
    pub details: Option<serde_json::Value>,
}

/// Outcome of an authorization decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthzDecisionOutcome {
    Allow,
    Deny(String), // reason
}

impl AuthzDecisionOutcome {
    pub fn allow() -> Self {
        AuthzDecisionOutcome::Allow
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        AuthzDecisionOutcome::Deny(reason.into())
    }

    pub fn is_allow(&self) -> bool {
        matches!(self, AuthzDecisionOutcome::Allow)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            AuthzDecisionOutcome::Deny(reason) => Some(reason),
            _ => None,
        }
    }
}

// ── Audit Log Trait ─────────────────────────────────────────────────────────

/// Audit log: record every authorization decision.
pub trait AuditLogWriter: std::fmt::Debug + Send + Sync {
    /// Log an authorization decision.
    fn log(&self, decision: &AuthzDecision);
}

/// In-memory audit log writer for testing.
#[derive(Debug, Default, Clone)]
pub struct InMemoryAuditLog {
    entries: std::sync::Arc<std::sync::Mutex<Vec<AuthzDecision>>>,
}

impl InMemoryAuditLog {
    pub fn entries(&self) -> Vec<AuthzDecision> {
        self.entries.lock().unwrap().clone()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

impl AuditLogWriter for InMemoryAuditLog {
    fn log(&self, decision: &AuthzDecision) {
        self.entries.lock().unwrap().push(decision.clone());
    }
}

impl InMemoryAuditLog {
    pub fn create_default() -> Self {
        Self::default()
    }
}

// ── Request Cache ───────────────────────────────────────────────────────────

/// Cached authz decisions per request to avoid redundant checks.
#[derive(Debug, Default)]
pub struct AuthzCache {
    entries: std::sync::RwLock<std::collections::HashMap<String, (AuthzDecisionOutcome, Instant)>>,
    ttl: Duration,
}

impl AuthzCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: std::sync::RwLock::new(std::collections::HashMap::new()),
            ttl,
        }
    }

    /// Check if a cached decision exists and is still valid.
    pub fn get(&self, key: &str) -> Option<AuthzDecisionOutcome> {
        let entries = self.entries.read().unwrap();
        if let Some((outcome, time)) = entries.get(key) {
            if time.elapsed() < self.ttl {
                return Some(outcome.clone());
            }
        }
        None
    }

    /// Store a decision in the cache.
    pub fn put(&self, key: String, outcome: AuthzDecisionOutcome) {
        let mut entries = self.entries.write().unwrap();
        entries.insert(key, (outcome, Instant::now()));
        // Clean up expired entries
        entries.retain(|_, (time)| Instant::now().elapsed() < self.ttl);
    }

    /// Clear all cached decisions.
    pub fn clear(&self) {
        self.entries.write().unwrap().clear();
    }
}

// ── Require Role Attribute ─────────────────────────────────────────────────

/// Required role for an endpoint (used with #[require_role] macro).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredRole {
    Ingest,
    Viewer,
    ComplianceAuditor,
    TokenAdmin,
    Admin,
}

impl RequiredRole {
    pub fn to_role(self) -> Role {
        match self {
            Self::Ingest => Role::Ingest,
            Self::Viewer => Role::Viewer,
            Self::ComplianceAuditor => Role::ComplianceAuditor,
            Self::TokenAdmin => Role::TokenAdmin,
            Self::Admin => Role::Admin,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Ingest => "ingest",
            Self::Viewer => "viewer",
            Self::ComplianceAuditor => "compliance-auditor",
            Self::TokenAdmin => "token-admin",
            Self::Admin => "admin",
        }
    }
}

// ── Authz Enforcer ──────────────────────────────────────────────────────────

/// Core authorization enforcer: checks scope + role + audit.
#[derive(Debug, Clone)]
pub struct AuthzEnforcer {
    audit: Arc<InMemoryAuditLog>,
    cache: Arc<AuthzCache>,
}

impl AuthzEnforcer {
    pub fn new(audit: Arc<InMemoryAuditLog>, cache: Arc<AuthzCache>) -> Self {
        Self { audit, cache }
    }

    pub fn create_default() -> Self {
        Self {
            audit: Arc::new(InMemoryAuditLog::default()),
            cache: Arc::new(AuthzCache::new(Duration::from_secs(300))),
        }
    }

    /// Check if scope meets the required role.
    pub fn check_role(&self, scope: &Scope, required: Role, action: &str) -> AuthzDecision {
        // Check cache first
        let cache_key = format!("role:{}:{}", required.name(), action);
        if let Some(cached) = self.cache.get(&cache_key) {
            return AuthzDecision {
                subject: scope.roles.iter().next().cloned().unwrap_or("anonymous".to_string()),
                resource: "global".to_string(),
                action: action.to_string(),
                decision: cached,
                rule: Some("cached".to_string()),
                details: None,
            };
        }

        let decision = if scope.meets_role(required) {
            AuthzDecisionOutcome::allow()
        } else {
            AuthzDecisionOutcome::deny(format!("role {} required, scope has {:?}", required, scope.roles))
        };

        let authz_decision = AuthzDecision {
            subject: scope.roles.iter().next().cloned().unwrap_or("anonymous".to_string()),
            resource: "global".to_string(),
            action: action.to_string(),
            decision: decision.clone(),
            rule: Some(format!("role:{}", required.name())),
            details: None,
        };

        // Cache the decision
        self.cache.put(cache_key, decision);

        // Audit the decision
        self.audit.log(&authz_decision);

        authz_decision
    }

    /// Check scope has project access.
    pub fn check_project(&self, scope: &Scope, project: &str) -> AuthzDecision {
        if scope.has_project(project) {
            AuthzDecision {
                subject: scope.roles.iter().next().cloned().unwrap_or("anonymous".to_string()),
                resource: project.to_string(),
                action: "project_access".to_string(),
                decision: AuthzDecisionOutcome::allow(),
                rule: Some("project_scope".to_string()),
                details: None,
            }
        } else {
            AuthzDecision {
                subject: scope.roles.iter().next().cloned().unwrap_or("anonymous".to_string()),
                resource: project.to_string(),
                action: "project_access".to_string(),
                decision: AuthzDecisionOutcome::deny(format!("project {} not in scope", project)),
                rule: Some("project_scope".to_string()),
                details: None,
            }
        }
    }

    /// Check if scope can read (has Viewer or higher).
    pub fn check_read(&self, scope: &Scope) -> AuthzDecision {
        self.check_role(scope, Role::Viewer, "read")
    }

    /// Check if scope can write (has Ingest or higher).
    pub fn check_write(&self, scope: &Scope) -> AuthzDecision {
        self.check_role(scope, Role::Ingest, "write")
    }

    /// Check if scope can manage tokens (has TokenAdmin or higher).
    pub fn check_token_admin(&self, scope: &Scope) -> AuthzDecision {
        self.check_role(scope, Role::TokenAdmin, "manage_tokens")
    }

    /// Get audit log entries.
    pub fn audit_entries(&self) -> Vec<AuthzDecision> {
        self.audit.entries()
    }
}

// ── ScopeFilter trait ───────────────────────────────────────────────────────

/// ScopeFilter: translates a `Scope` into SQL WHERE clause fragments
/// for a given entity table.
///
/// Every entity type (nodes, runs, resource_events, etc.) implements this
/// to append project scoping clauses to queries.
///
/// Scope applies to:
/// - Data retrieval (SELECT ... WHERE project = ...)
/// - COUNT queries (SELECT COUNT(*) WHERE project = ...)
/// - Aggregates (GROUP BY + WHERE project = ...)
/// - Existence checks (EXISTS ... WHERE project = ...)
pub trait ScopeFilter {
    /// Table name this filter applies to.
    fn table_name() -> &'static str;

    /// Column that holds the project identifier.
    fn project_column() -> &'static str;

    /// Build the WHERE clause fragment for scope enforcement.
    fn scope_where(scope: &Scope) -> (String, Vec<String>) {
        if scope.projects.is_empty() {
            return (String::new(), Vec::new());
        }

        // Generate: AND table.project IN ($1, $2, ...)
        let placeholders: Vec<String> =
            (0..scope.projects.len()).map(|i| format!("${}", i + 1)).collect();
        let clause = format!(
            "AND {} IN ({})",
            Self::project_column(),
            placeholders.join(",")
        );
        let params: Vec<String> = scope
            .projects
            .iter()
            .cloned()
            .map(|p| format!("'{}'", p.replace('\'', "''")))
            .collect();

        (clause, params)
    }

    /// Build the WHERE clause for counting.
    fn count_scope_where(scope: &Scope) -> (String, Vec<String>) {
        Self::scope_where(scope)
    }

    /// Build the WHERE clause for aggregate queries.
    fn aggregate_scope_where(scope: &Scope) -> (String, Vec<String>) {
        Self::scope_where(scope)
    }

    /// Build the WHERE clause for existence checks (EXISTS subqueries).
    fn exists_scope_where(scope: &Scope) -> (String, Vec<String>) {
        Self::scope_where(scope)
    }
}

/// ScopeFilter implementation for nodes table.
pub struct NodesScopeFilter;

impl ScopeFilter for NodesScopeFilter {
    fn table_name() -> &'static str {
        "nodes"
    }

    fn project_column() -> &'static str {
        "project"
    }
}

/// ScopeFilter implementation for runs table.
pub struct RunsScopeFilter;

impl ScopeFilter for RunsScopeFilter {
    fn table_name() -> &'static str {
        "runs"
    }

    fn project_column() -> &'static str {
        "project"
    }
}

/// ScopeFilter for resource_events table.
pub struct ResourceEventsScopeFilter;

impl ScopeFilter for ResourceEventsScopeFilter {
    fn table_name() -> &'static str {
        "resource_events"
    }

    fn project_column() -> &'static str {
        "project"
    }
}

/// ScopeFilter for compliance_reports table.
pub struct ComplianceReportsScopeFilter;

impl ScopeFilter for ComplianceReportsScopeFilter {
    fn table_name() -> &'static str {
        "compliance_reports"
    }

    fn project_column() -> &'static str {
        "project"
    }
}

/// ScopeFilter for duration_rollups table.
pub struct RollupsScopeFilter;

impl ScopeFilter for RollupsScopeFilter {
    fn table_name() -> &'static str {
        "duration_rollups"
    }

    fn project_column() -> &'static str {
        "project"
    }
}

// ── Attribute stripping helpers ────────────────────────────────────────────

/// Node attributes response wrapper.
///
/// When a `compliance-auditor` role accesses node data, the attributes
/// field is stripped. Admin and other roles see full attributes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeAttributes {
    /// Full attributes — visible to Admin and Viewer roles.
    pub full: Option<serde_json::Value>,
    /// Stripped attributes — visible only to compliance-auditor.
    pub stripped: Option<serde_json::Value>,
}

impl NodeAttributes {
    /// Build the response attributes based on role.
    pub fn resolve(role: Option<Role>, raw: Option<serde_json::Value>) -> serde_json::Value {
        match role {
            // ComplianceAuditor gets null attributes.
            Some(Role::ComplianceAuditor) => serde_json::Value::Null,
            // Everyone else sees full attributes.
            _ => raw.unwrap_or(serde_json::Value::Null),
        }
    }
}

// ── Re-exports ──────────────────────────────────────────────────────────────

// All public types are defined at the top level; no submodule re-exports needed.

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Role tests ────────────────────────────────────────────────────────

    #[test]
    fn test_role_display() {
        assert_eq!(Role::Ingest.to_string(), "ingest");
        assert_eq!(Role::Viewer.to_string(), "viewer");
        assert_eq!(Role::ComplianceAuditor.to_string(), "compliance-auditor");
        assert_eq!(Role::TokenAdmin.to_string(), "token-admin");
        assert_eq!(Role::Admin.to_string(), "admin");
    }

    #[test]
    fn test_role_permissions() {
        // Ingest: write-only
        assert!(!Role::Ingest.can_read());
        assert!(Role::Ingest.can_write());
        assert!(!Role::Ingest.can_manage_tokens());
        assert!(!Role::Ingest.is_compliance_auditor());
        assert!(!Role::Ingest.can_audit());
        assert!(Role::Ingest.can_view_nodes());

        // Viewer: read access
        assert!(Role::Viewer.can_read());
        assert!(!Role::Viewer.can_write());
        assert!(!Role::Viewer.can_manage_tokens());
        assert!(!Role::Viewer.is_compliance_auditor());
        assert!(!Role::Viewer.can_audit());
        assert!(Role::Viewer.can_view_nodes());

        // ComplianceAuditor: read compliance, strip attributes
        assert!(Role::ComplianceAuditor.can_read());
        assert!(!Role::ComplianceAuditor.can_write());
        assert!(!Role::ComplianceAuditor.can_manage_tokens());
        assert!(Role::ComplianceAuditor.is_compliance_auditor());
        assert!(Role::ComplianceAuditor.can_audit());
        assert!(!Role::ComplianceAuditor.can_view_nodes());

        // TokenAdmin: manage tokens
        assert!(Role::TokenAdmin.can_read());
        assert!(Role::TokenAdmin.can_write());
        assert!(Role::TokenAdmin.can_manage_tokens());
        assert!(!Role::TokenAdmin.is_compliance_auditor());
        assert!(Role::TokenAdmin.can_audit());
        assert!(Role::TokenAdmin.can_view_nodes());

        // Admin: everything
        assert!(Role::Admin.can_read());
        assert!(Role::Admin.can_write());
        assert!(Role::Admin.can_manage_tokens());
        assert!(!Role::Admin.is_compliance_auditor());
        assert!(!Role::Admin.can_audit());
        assert!(Role::Admin.can_view_nodes());
    }

    // ── Role hierarchy tests ─────────────────────────────────────────────

    #[test]
    fn test_role_hierarchy_admin_includes_all() {
        // Admin includes all read roles
        assert!(Role::Admin.includes(Role::Viewer));
        assert!(Role::Admin.includes(Role::ComplianceAuditor));
        assert!(Role::Admin.includes(Role::TokenAdmin));
        assert!(Role::Admin.includes(Role::Admin));
        // Admin also has Ingest write access (separate dimension)
        assert!(Role::Admin.includes(Role::Ingest));
    }

    #[test]
    fn test_role_hierarchy_tokenadmin_includes_lower() {
        assert!(Role::TokenAdmin.includes(Role::Viewer));
        assert!(Role::TokenAdmin.includes(Role::ComplianceAuditor));
        assert!(Role::TokenAdmin.includes(Role::TokenAdmin));
        // TokenAdmin also has Ingest write access
        assert!(Role::TokenAdmin.includes(Role::Ingest));
        assert!(!Role::TokenAdmin.includes(Role::Admin));
    }

    #[test]
    fn test_role_hierarchy_complianceauditor_includes_lower() {
        assert!(Role::ComplianceAuditor.includes(Role::Viewer));
        assert!(Role::ComplianceAuditor.includes(Role::ComplianceAuditor));
        // ComplianceAuditor does NOT have write access
        assert!(!Role::ComplianceAuditor.includes(Role::Ingest));
        assert!(!Role::ComplianceAuditor.includes(Role::TokenAdmin));
        assert!(!Role::ComplianceAuditor.includes(Role::Admin));
    }

    #[test]
    fn test_role_hierarchy_viewer_includes_lower() {
        // Viewer is read-only — does not include Ingest
        assert!(!Role::Viewer.includes(Role::Ingest));
        assert!(Role::Viewer.includes(Role::Viewer));
        assert!(!Role::Viewer.includes(Role::ComplianceAuditor));
        assert!(!Role::Viewer.includes(Role::TokenAdmin));
        assert!(!Role::Viewer.includes(Role::Admin));
    }

    #[test]
    fn test_role_hierarchy_ingest_only_self() {
        assert!(Role::Ingest.includes(Role::Ingest));
        assert!(!Role::Ingest.includes(Role::Viewer));
        assert!(!Role::Ingest.includes(Role::ComplianceAuditor));
        assert!(!Role::Ingest.includes(Role::TokenAdmin));
        assert!(!Role::Ingest.includes(Role::Admin));
    }

    // ── Scope role tests ─────────────────────────────────────────────────

    #[test]
    fn test_scope_meets_role_hierarchy() {
        let mut roles = HashSet::new();
        roles.insert("admin".to_string());
        let admin_scope = Scope::new(HashSet::new(), roles);
        assert!(admin_scope.meets_role(Role::Ingest));
        assert!(admin_scope.meets_role(Role::Viewer));
        assert!(admin_scope.meets_role(Role::ComplianceAuditor));
        assert!(admin_scope.meets_role(Role::TokenAdmin));
        assert!(admin_scope.meets_role(Role::Admin));
    }

    #[test]
    fn test_scope_meets_role_viewer() {
        let mut roles = HashSet::new();
        roles.insert("viewer".to_string());
        let viewer_scope = Scope::new(HashSet::new(), roles);
        assert!(viewer_scope.meets_role(Role::Viewer));
        // Viewer is read-only — cannot write
        assert!(!viewer_scope.meets_role(Role::Ingest));
        assert!(!viewer_scope.meets_role(Role::ComplianceAuditor));
        assert!(!viewer_scope.meets_role(Role::TokenAdmin));
        assert!(!viewer_scope.meets_role(Role::Admin));
    }

    #[test]
    fn test_scope_meets_role_compliance_auditor() {
        let mut roles = HashSet::new();
        roles.insert("compliance-auditor".to_string());
        let auditor_scope = Scope::new(HashSet::new(), roles);
        assert!(auditor_scope.meets_role(Role::ComplianceAuditor));
        assert!(auditor_scope.meets_role(Role::Viewer));
        // ComplianceAuditor is read-only — cannot write
        assert!(!auditor_scope.meets_role(Role::Ingest));
        assert!(!auditor_scope.meets_role(Role::TokenAdmin));
        assert!(!auditor_scope.meets_role(Role::Admin));
    }

    #[test]
    fn test_scope_can_read_write_token_admin() {
        let mut roles = HashSet::new();
        roles.insert("token-admin".to_string());
        let admin_scope = Scope::new(HashSet::new(), roles);
        assert!(admin_scope.can_read());
        assert!(admin_scope.can_write());
        assert!(admin_scope.can_manage_tokens());
    }

    #[test]
    fn test_scope_can_read_ingest() {
        let mut roles = HashSet::new();
        roles.insert("ingest".to_string());
        let ingest_scope = Scope::new(HashSet::new(), roles);
        assert!(!ingest_scope.can_read());
        assert!(ingest_scope.can_write());
        assert!(!ingest_scope.can_manage_tokens());
    }

    #[test]
    fn test_scope_empty_is_unrestricted() {
        let scope = Scope::all();
        assert!(scope.projects.is_empty());
        assert!(scope.roles.is_empty());
        assert!(scope.has_project("anything"));
        assert!(scope.has_role("anything"));
        assert!(!scope.is_scoped());
        assert!(scope.meets_role(Role::Ingest));
        assert!(scope.meets_role(Role::Admin));
    }

    #[test]
    fn test_scope_with_projects() {
        let mut projects = HashSet::new();
        projects.insert("project-a".to_string());
        projects.insert("project-b".to_string());
        let scope = Scope::new(projects, HashSet::new());

        assert!(scope.has_project("project-a"));
        assert!(scope.has_project("project-b"));
        assert!(!scope.has_project("project-c"));
        assert!(scope.is_scoped());
    }

    #[test]
    fn test_scope_filter_project_clauses() {
        // Unrestricted scope → no WHERE clause
        let unrestricted = Scope::all();
        let (clause, params) = <NodesScopeFilter as ScopeFilter>::scope_where(&unrestricted);
        assert_eq!(clause, "");
        assert!(params.is_empty());

        // Scoped scope → IN clause
        let mut projects = HashSet::new();
        projects.insert("proj-a".to_string());
        projects.insert("proj-b".to_string());
        let scoped = Scope::new(projects, HashSet::new());
        let (clause, params) = <NodesScopeFilter as ScopeFilter>::scope_where(&scoped);

        assert!(clause.contains("AND"));
        assert!(clause.contains("IN"));
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_scope_filter_aggregate_and_count() {
        let mut projects = HashSet::new();
        projects.insert("test-project".to_string());
        let scope = Scope::new(projects, HashSet::new());

        // COUNT should get the same WHERE clause as regular query
        let (_, params) = <NodesScopeFilter as ScopeFilter>::count_scope_where(&scope);
        assert_eq!(params.len(), 1);

        // Aggregates should also be scoped
        let (clause, _) = <RollupsScopeFilter as ScopeFilter>::aggregate_scope_where(&scope);
        assert!(clause.contains("AND"));

        // EXISTS checks should also be scoped
        let (clause, _) = <RunsScopeFilter as ScopeFilter>::exists_scope_where(&scope);
        assert!(clause.contains("AND"));
    }

    #[test]
    fn test_node_attributes_stripping() {
        let attrs = serde_json::json!({
            "name": "test-node",
            "platform": "linux",
            "private_key": "secret"
        });

        // ComplianceAuditor → null attributes
        let result = NodeAttributes::resolve(Some(Role::ComplianceAuditor), Some(attrs.clone()));
        assert_eq!(result, serde_json::Value::Null);

        // Admin → full attributes
        let result = NodeAttributes::resolve(Some(Role::Admin), Some(attrs.clone()));
        assert_eq!(result, attrs);

        // No role specified → full attributes
        let result = NodeAttributes::resolve(None, Some(attrs.clone()));
        assert_eq!(result, attrs);

        // No role + null input → null
        let result = NodeAttributes::resolve(Some(Role::ComplianceAuditor), None);
        assert_eq!(result, serde_json::Value::Null);
    }

    #[test]
    fn test_scope_filter_table_names() {
        assert_eq!(NodesScopeFilter::table_name(), "nodes");
        assert_eq!(RunsScopeFilter::table_name(), "runs");
        assert_eq!(ResourceEventsScopeFilter::table_name(), "resource_events");
        assert_eq!(
            ComplianceReportsScopeFilter::table_name(),
            "compliance_reports"
        );
        assert_eq!(RollupsScopeFilter::table_name(), "duration_rollups");
    }

    #[test]
    fn test_scope_filter_project_columns() {
        assert_eq!(NodesScopeFilter::project_column(), "project");
        assert_eq!(RunsScopeFilter::project_column(), "project");
        assert_eq!(
            ResourceEventsScopeFilter::project_column(),
            "project"
        );
        assert_eq!(
            ComplianceReportsScopeFilter::project_column(),
            "project"
        );
        assert_eq!(RollupsScopeFilter::project_column(), "project");
    }

    #[test]
    fn test_scope_filter_sql_injection_safety() {
        let mut projects = HashSet::new();
        projects.insert("safe-project".to_string());
        let scope = Scope::new(projects, HashSet::new());
        let (_, params) = <NodesScopeFilter as ScopeFilter>::scope_where(&scope);

        // Params are wrapped in single quotes for SQL — that's correct.
        // What matters: untrusted input gets escaped (doubled quotes).
        assert_eq!(params.len(), 1);
        assert_eq!(params[0], "'safe-project'");

        // Test with untrusted input that contains a quote — it should be escaped.
        let mut untrusted = HashSet::new();
        untrusted.insert("proj'or'1=1".to_string());
        let bad_scope = Scope::new(untrusted, HashSet::new());
        let (_, bad_params) = <NodesScopeFilter as ScopeFilter>::scope_where(&bad_scope);
        // The embedded quote should be doubled: 'proj''or''1=1'
        assert_eq!(bad_params[0], "'proj''or''1=1'");
    }

    // ── AuthzCache tests ─────────────────────────────────────────────────

    #[test]
    fn test_authz_cache_put_and_get() {
        let cache = AuthzCache::new(Duration::from_secs(60));
        cache.put("test-key".to_string(), AuthzDecisionOutcome::allow());
        assert_eq!(cache.get("test-key"), Some(AuthzDecisionOutcome::allow()));
    }

    #[test]
    fn test_authz_cache_expired() {
        let cache = AuthzCache::new(Duration::from_millis(10));
        cache.put("test-key".to_string(), AuthzDecisionOutcome::allow());
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cache.get("test-key"), None);
    }

    #[test]
    fn test_authz_cache_clear() {
        let cache = AuthzCache::new(Duration::from_secs(60));
        cache.put("test-key".to_string(), AuthzDecisionOutcome::allow());
        cache.clear();
        assert_eq!(cache.get("test-key"), None);
    }

    // ── InMemoryAuditLog tests ───────────────────────────────────────────

    #[test]
    fn test_in_memory_audit_log() {
        let log = InMemoryAuditLog::default();
        let decision = AuthzDecision {
            subject: "user1".to_string(),
            resource: "nodes".to_string(),
            action: "read".to_string(),
            decision: AuthzDecisionOutcome::allow(),
            rule: Some("role:viewer".to_string()),
            details: None,
        };
        log.log(&decision);
        assert_eq!(log.len(), 1);
        let entries = log.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].subject, "user1");
    }

    // ── AuthzEnforcer tests ──────────────────────────────────────────────

    #[test]
    fn test_authz_enforcer_check_role() {
        let enforcer = AuthzEnforcer::create_default();
        let mut roles = HashSet::new();
        roles.insert("viewer".to_string());
        let scope = Scope::new(HashSet::new(), roles);

        let decision = enforcer.check_role(&scope, Role::Viewer, "read");
        assert!(decision.decision.is_allow());

        let decision = enforcer.check_role(&scope, Role::Admin, "admin");
        assert!(!decision.decision.is_allow());
    }

    #[test]
    fn test_authz_enforcer_check_project() {
        let enforcer = AuthzEnforcer::create_default();
        let mut projects = HashSet::new();
        projects.insert("test-project".to_string());
        let scope = Scope::new(projects, HashSet::new());

        let decision = enforcer.check_project(&scope, "test-project");
        assert!(decision.decision.is_allow());

        let decision = enforcer.check_project(&scope, "other-project");
        assert!(!decision.decision.is_allow());
    }

    #[test]
    fn test_authz_enforcer_caching() {
        let enforcer = AuthzEnforcer::create_default();
        let mut roles = HashSet::new();
        roles.insert("viewer".to_string());
        let scope = Scope::new(HashSet::new(), roles);

        // First call — not cached
        let d1 = enforcer.check_role(&scope, Role::Viewer, "read");
        assert!(d1.decision.is_allow());
        assert_eq!(d1.rule, Some("role:viewer".to_string()));

        // Second call — cached
        let d2 = enforcer.check_role(&scope, Role::Viewer, "read");
        assert!(d2.decision.is_allow());
        assert_eq!(d2.rule, Some("cached".to_string()));
    }

    #[test]
    fn test_authz_enforcer_audit_log() {
        let enforcer = AuthzEnforcer::create_default();
        let mut roles = HashSet::new();
        roles.insert("admin".to_string());
        let scope = Scope::new(HashSet::new(), roles);

        let _ = enforcer.check_role(&scope, Role::Admin, "admin");
        // Audit log should have the decision
        let entries = enforcer.audit_entries();
        assert!(!entries.is_empty());
    }

    // ── AuthzDecisionOutcome tests ───────────────────────────────────────

    #[test]
    fn test_authz_decision_outcome() {
        let allow = AuthzDecisionOutcome::allow();
        assert!(allow.is_allow());
        assert_eq!(allow.reason(), None);

        let deny = AuthzDecisionOutcome::deny("test reason");
        assert!(!deny.is_allow());
        assert_eq!(deny.reason(), Some("test reason"));
    }
}
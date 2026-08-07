//! spindle-authz: Authorization types and traits for query-layer scoping.
//!
//! Per PLANS.md M2-12:
//! - `Scope` struct: projects: HashSet<String>, roles: HashSet<Role>
//! - Every store method requires &Scope — fails to compile without it
//! - `ScopeFilter` trait: translates scope to SQL WHERE clause
//! - `compliance-auditor` → node attributes stripped at store layer
//! - Scope applies to COUNT, aggregates, existence checks — not just data retrieval

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

// ── Roles ───────────────────────────────────────────────────────────────────

/// Spindle authorization roles.
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

        // Viewer: read access
        assert!(Role::Viewer.can_read());
        assert!(!Role::Viewer.can_write());
        assert!(!Role::Viewer.can_manage_tokens());
        assert!(!Role::Viewer.is_compliance_auditor());

        // ComplianceAuditor: read compliance, strip attributes
        assert!(Role::ComplianceAuditor.can_read());
        assert!(!Role::ComplianceAuditor.can_write());
        assert!(!Role::ComplianceAuditor.can_manage_tokens());
        assert!(Role::ComplianceAuditor.is_compliance_auditor());

        // TokenAdmin: manage tokens
        assert!(Role::TokenAdmin.can_read());
        assert!(Role::TokenAdmin.can_write());
        assert!(Role::TokenAdmin.can_manage_tokens());
        assert!(!Role::TokenAdmin.is_compliance_auditor());

        // Admin: everything
        assert!(Role::Admin.can_read());
        assert!(Role::Admin.can_write());
        assert!(Role::Admin.can_manage_tokens());
        assert!(!Role::Admin.is_compliance_auditor());
        assert!(Role::Admin.is_admin());
    }

    #[test]
    fn test_scope_empty_is_unrestricted() {
        let scope = Scope::all();
        assert!(scope.projects.is_empty());
        assert!(scope.roles.is_empty());
        assert!(scope.has_project("anything"));
        assert!(scope.has_role("anything"));
        assert!(!scope.is_scoped());
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
    fn test_scope_with_roles() {
        let mut roles = HashSet::new();
        roles.insert("viewer".to_string());
        let scope = Scope::new(HashSet::new(), roles);

        assert!(scope.has_role("viewer"));
        assert!(!scope.has_role("admin"));
    }

    #[test]
    fn test_scope_compliance_auditor() {
        let mut roles = HashSet::new();
        roles.insert("compliance-auditor".to_string());
        let scope = Scope::new(HashSet::new(), roles);

        assert!(scope.is_compliance_auditor());
        assert!(!scope.is_admin());
    }

    #[test]
    fn test_scope_admin() {
        let mut roles = HashSet::new();
        roles.insert("admin".to_string());
        let scope = Scope::new(HashSet::new(), roles);

        assert!(scope.is_admin());
        assert!(!scope.is_compliance_auditor());
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
}
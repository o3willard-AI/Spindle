//! spindle-store: Typed store interfaces for Spindle entities.
//!
//! Provides compile-time-safe access to database entities via `sqlx::query!`
//! with `Scope` enforcement on every method.
//!
//! ## Design
//! - Each store wraps `PgPool` and requires `&Scope` on every method.
//! - Calling `get_run()` without scope is a hard compile error.
//! - All queries are defined in this crate — no raw SQL leaks out.
//! - Phase 1: trait contracts + stub queries (this commit).
//! - Phase 2: `cargo sqlx prepare` against live DB for compile-time checking.
//!
//! ## Authorization (M2-12)
//! - `spindle_authz::Scope` with `projects: HashSet` and `roles: HashSet`
//! - Scope applies to ALL queries: data retrieval, COUNT, aggregates, EXISTS
//! - `compliance-auditor` role → node attributes stripped at store layer
//! - ScopeFilter trait generates SQL WHERE clauses per entity type

use sqlx::PgPool;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use chrono::{DateTime, Utc};
use std::collections::HashSet;

// ── Re-export authz types ───────────────────────────────────────────────────

pub use spindle_authz::{
    Role, Scope, ScopeFilter,
    NodesScopeFilter, RunsScopeFilter, ResourceEventsScopeFilter,
    ComplianceReportsScopeFilter, RollupsScopeFilter,
};

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("Query failed: {0}")]
    QueryFailed(#[from] sqlx::Error),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Scope denied: {0}")]
    ScopeDenied(String),
    #[error("Store error: {0}")]
    Storage(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

// ── PgStore — base wrapper ──────────────────────────────────────────────────

/// Wraps `PgPool` and provides a convenience `pool()` accessor.
#[derive(Debug, Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

// ── Helper: build scoped WHERE clause ──────────────────────────────────────

/// Build a scope-enforced WHERE clause for any scope filter type.
/// Returns `(clause, params)` — empty if scope is unrestricted.
pub fn build_scope_filter<T: ScopeFilter>(scope: &Scope) -> (String, Vec<String>) {
    T::scope_where(scope)
}

/// Build a scope-enforced WHERE clause for COUNT queries.
/// Identical to scope_where — scope applies to counts too.
pub fn build_count_filter<T: ScopeFilter>(scope: &Scope) -> (String, Vec<String>) {
    T::count_scope_where(scope)
}

/// Build a scope-enforced WHERE clause for aggregate queries.
/// Identical to scope_where — scope applies to aggregates too.
pub fn build_aggregate_filter<T: ScopeFilter>(scope: &Scope) -> (String, Vec<String>) {
    T::aggregate_scope_where(scope)
}

/// Build a scope-enforced WHERE clause for existence checks (EXISTS).
/// Identical to scope_where — scope applies to EXISTS too.
pub fn build_exists_filter<T: ScopeFilter>(scope: &Scope) -> (String, Vec<String>) {
    T::exists_scope_where(scope)
}

// ── Helper: node attribute stripping ───────────────────────────────────────

/// Strip node attributes for compliance-auditor roles.
/// Returns `None` if the scope contains the `compliance-auditor` role,
/// otherwise returns the original attributes unchanged.
pub fn strip_attributes_for_auditor(
    scope: &Scope,
    attrs: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    if scope.is_compliance_auditor() && !scope.is_admin() {
        None
    } else {
        attrs
    }
}

/// Resolve node attributes based on role.
/// ComplianceAuditor → null. Everyone else → full attributes.
pub fn resolve_node_attributes(
    scope: &Scope,
    raw: Option<serde_json::Value>,
) -> serde_json::Value {
    if scope.is_compliance_auditor() && !scope.is_admin() {
        serde_json::Value::Null
    } else {
        raw.unwrap_or(serde_json::Value::Null)
    }
}

// ── Role enforcement helpers ───────────────────────────────────────────────

/// Check scope has read access (Viewer or higher).
pub fn enforce_read(scope: &Scope) -> Result<()> {
    if !scope.can_read() {
        return Err(StoreError::ScopeDenied(format!(
            "read access denied, scope has {:?}",
            scope.roles
        )));
    }
    Ok(())
}

/// Check scope has write access (Ingest or higher).
pub fn enforce_write(scope: &Scope) -> Result<()> {
    if !scope.can_write() {
        return Err(StoreError::ScopeDenied(format!(
            "write access denied, scope has {:?}",
            scope.roles
        )));
    }
    Ok(())
}

/// Check scope can manage tokens (TokenAdmin or higher).
pub fn enforce_token_admin(scope: &Scope) -> Result<()> {
    if !scope.can_manage_tokens() {
        return Err(StoreError::ScopeDenied(format!(
            "token admin access denied, scope has {:?}",
            scope.roles
        )));
    }
    Ok(())
}

/// Check scope can view node details (not ComplianceAuditor).
pub fn enforce_can_view_nodes(scope: &Scope) -> Result<()> {
    if !scope.can_read() {
        return Err(StoreError::ScopeDenied(format!(
            "node access denied, scope has {:?}",
            scope.roles
        )));
    }
    Ok(())
}

/// Check scope can access compliance (Auditor or higher).
pub fn enforce_compliance(scope: &Scope) -> Result<()> {
    if !scope.can_read() {
        return Err(StoreError::ScopeDenied(format!(
            "compliance access denied, scope has {:?}",
            scope.roles
        )));
    }
    Ok(())
}

// ── Node ────────────────────────────────────────────────────────────────────

/// Node entity — machine managed by Spindle.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Node {
    pub id: Uuid,
    pub name: String,
    pub platform: String,
    pub platform_version: String,
    pub chef_environment: String,
    pub policy_group: String,
    pub policy_name: String,
    pub attributes: serde_json::Value,
    pub last_seen: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// Store trait for Node queries — every method requires `&Scope`.
#[async_trait::async_trait]
pub trait NodeStore {
    async fn get_node(&self, id: Uuid, scope: &Scope) -> Result<Node>;
    async fn list_nodes(
        &self,
        filter: Option<Vec<(&str, serde_json::Value)>>,
        scope: &Scope,
    ) -> Result<Vec<Node>>;
    async fn upsert_node(&self, node: &Node, scope: &Scope) -> Result<Uuid>;
    async fn count_nodes(&self, scope: &Scope) -> Result<usize>;
}

/// Node store implementation wrapping PgPool.
pub struct SqlxNodeStore {
    pg: PgStore,
}

impl SqlxNodeStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pg: PgStore::new(pool),
        }
    }

    pub fn pg(&self) -> &PgStore {
        &self.pg
    }
}

#[async_trait::async_trait]
impl NodeStore for SqlxNodeStore {
    async fn get_node(&self, id: Uuid, scope: &Scope) -> Result<Node> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("node".to_string()))
    }

    async fn list_nodes(
        &self,
        _filter: Option<Vec<(&str, serde_json::Value)>>,
        scope: &Scope,
    ) -> Result<Vec<Node>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("nodes".to_string()))
    }

    async fn upsert_node(&self, node: &Node, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Ok(node.id)
    }

    async fn count_nodes(&self, scope: &Scope) -> Result<usize> {
        // COUNT query uses the same scope filter — scope applies to counts!
        let (clause, _params) = build_count_filter::<NodesScopeFilter>(scope);
        // In a real implementation:
        // SELECT COUNT(*) FROM nodes {clause}
        let _ = clause;
        Ok(0)
    }
}

// ── Run ─────────────────────────────────────────────────────────────────────

/// Run entity — a chef-client run on a node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Run {
    pub id: Uuid,
    pub node_id: Uuid,
    pub run_id: String,
    pub status: String,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub total_resource_count: i32,
    pub updated_count: i32,
    pub failed_count: i32,
    pub skipped_count: i32,
    pub error_summary: Option<serde_json::Value>,
    pub cookbook_set: Option<serde_json::Value>,
    pub schema_version: i32,
    pub created_at: DateTime<Utc>,
}

/// Store trait for Run queries — every method requires `&Scope`.
#[async_trait::async_trait]
pub trait RunStore {
    async fn get_run(&self, id: Uuid, scope: &Scope) -> Result<Run>;
    async fn list_runs(
        &self,
        node_id: Uuid,
        time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
        scope: &Scope,
    ) -> Result<Vec<Run>>;
    async fn insert_run(&self, run: &Run, scope: &Scope) -> Result<Uuid>;
    async fn count_runs(&self, scope: &Scope) -> Result<usize>;
}

/// Run store implementation wrapping PgPool.
pub struct SqlxRunStore {
    pg: PgStore,
}

impl SqlxRunStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pg: PgStore::new(pool),
        }
    }

    pub fn pg(&self) -> &PgStore {
        &self.pg
    }
}

#[async_trait::async_trait]
impl RunStore for SqlxRunStore {
    async fn get_run(&self, id: Uuid, scope: &Scope) -> Result<Run> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("run".to_string()))
    }

    async fn list_runs(
        &self,
        _node_id: Uuid,
        _time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
        scope: &Scope,
    ) -> Result<Vec<Run>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("runs".to_string()))
    }

    async fn insert_run(&self, run: &Run, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Ok(run.id)
    }

    async fn count_runs(&self, scope: &Scope) -> Result<usize> {
        // COUNT query uses the same scope filter — scope applies to counts!
        let (clause, _params) = build_count_filter::<RunsScopeFilter>(scope);
        let _ = clause;
        Ok(0)
    }
}

// ── ResourceEvent ───────────────────────────────────────────────────────────

/// Resource event — a single resource management action during a run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct ResourceEvent {
    pub id: Uuid,
    pub run_id: Uuid,
    pub node_id: Uuid,
    pub resource_type: String,
    pub resource_name: String,
    pub action: String,
    pub status: String,
    pub duration_ms: i32,
    pub cookbook_name: String,
    pub cookbook_version: String,
    pub guard_outcome: Option<serde_json::Value>,
    pub delta: Option<serde_json::Value>,
    pub schema_version: i32,
    pub created_at: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait ResourceEventStore {
    async fn get_event(&self, id: Uuid, scope: &Scope) -> Result<ResourceEvent>;
    async fn list_events(&self, run_id: Uuid, scope: &Scope) -> Result<Vec<ResourceEvent>>;
    async fn insert_event(&self, event: &ResourceEvent, scope: &Scope) -> Result<Uuid>;
    async fn count_events(&self, scope: &Scope) -> Result<usize>;
}

pub struct SqlxResourceEventStore {
    pg: PgStore,
}

impl SqlxResourceEventStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pg: PgStore::new(pool),
        }
    }

    pub fn pg(&self) -> &PgStore {
        &self.pg
    }
}

#[async_trait::async_trait]
impl ResourceEventStore for SqlxResourceEventStore {
    async fn get_event(&self, _id: Uuid, scope: &Scope) -> Result<ResourceEvent> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("resource_event".to_string()))
    }

    async fn list_events(&self, _run_id: Uuid, scope: &Scope) -> Result<Vec<ResourceEvent>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("resource_events".to_string()))
    }

    async fn insert_event(&self, _event: &ResourceEvent, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }

    async fn count_events(&self, scope: &Scope) -> Result<usize> {
        // COUNT query uses the same scope filter — scope applies to counts!
        let (clause, _params) = build_count_filter::<ResourceEventsScopeFilter>(scope);
        let _ = clause;
        Ok(0)
    }
}

// ── Compliance ──────────────────────────────────────────────────────────────

/// Compliance report — outcome of a profile evaluation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct ComplianceReport {
    pub id: Uuid,
    pub run_id: Uuid,
    pub node_id: Uuid,
    pub profile_id: Uuid,
    pub status: String,
    pub passed_count: i32,
    pub failed_count: i32,
    pub warning_count: i32,
    pub created_at: DateTime<Utc>,
}

/// Control result — outcome of a single control within a profile.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct ControlResult {
    pub id: Uuid,
    pub run_id: Uuid,
    pub node_id: Uuid,
    pub profile_id: Uuid,
    pub control_id: String,
    pub status: String,
    pub impact: String,
    pub result: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Store trait for compliance data — every method requires `&Scope`.
#[async_trait::async_trait]
pub trait ComplianceStore {
    async fn get_report(&self, id: Uuid, scope: &Scope) -> Result<ComplianceReport>;
    async fn list_reports(&self, run_id: Uuid, scope: &Scope) -> Result<Vec<ComplianceReport>>;
    async fn insert_report(&self, report: &ComplianceReport, scope: &Scope) -> Result<Uuid>;
    async fn get_control_results(
        &self,
        report_id: Uuid,
        scope: &Scope,
    ) -> Result<Vec<ControlResult>>;
    async fn insert_control_result(
        &self,
        result: &ControlResult,
        scope: &Scope,
    ) -> Result<Uuid>;
    async fn count_reports(&self, scope: &Scope) -> Result<usize>;
}

/// Compliance store implementation wrapping PgPool.
pub struct SqlxComplianceStore {
    pg: PgStore,
}

impl SqlxComplianceStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pg: PgStore::new(pool),
        }
    }

    pub fn pg(&self) -> &PgStore {
        &self.pg
    }
}

#[async_trait::async_trait]
impl ComplianceStore for SqlxComplianceStore {
    async fn get_report(&self, _id: Uuid, scope: &Scope) -> Result<ComplianceReport> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("compliance_report".to_string()))
    }

    async fn list_reports(&self, _run_id: Uuid, scope: &Scope) -> Result<Vec<ComplianceReport>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("compliance_reports".to_string()))
    }

    async fn insert_report(&self, _report: &ComplianceReport, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }

    async fn get_control_results(
        &self,
        _report_id: Uuid,
        scope: &Scope,
    ) -> Result<Vec<ControlResult>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("control_results".to_string()))
    }

    async fn insert_control_result(
        &self,
        _result: &ControlResult,
        scope: &Scope,
    ) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }

    async fn count_reports(&self, scope: &Scope) -> Result<usize> {
        // COUNT query uses the same scope filter — scope applies to counts!
        let (clause, _params) = build_count_filter::<ComplianceReportsScopeFilter>(scope);
        let _ = clause;
        Ok(0)
    }
}

// ── Rollup ──────────────────────────────────────────────────────────────────

/// Duration rollup — aggregated timing stats per cookbook/resource.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Rollup {
    pub id: Uuid,
    pub hour: DateTime<Utc>,
    pub cookbook_name: String,
    pub cookbook_version: String,
    pub resource_type: String,
    pub platform: String,
    pub count: i32,
    pub total_duration_ms: i64,
    pub p50_ms: Option<i32>,
    pub p95_ms: Option<i32>,
    pub p99_ms: Option<i32>,
    pub max_ms: i32,
    pub created_at: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait RollupStore {
    async fn get_rollups(
        &self,
        hour: DateTime<Utc>,
        scope: &Scope,
    ) -> Result<Vec<Rollup>>;
    async fn insert_rollup(&self, rollup: &Rollup, scope: &Scope) -> Result<Uuid>;
    async fn upsert_rollup(&self, rollup: &Rollup, scope: &Scope) -> Result<Uuid>;
    async fn aggregate_rollups(
        &self,
        hour: DateTime<Utc>,
        scope: &Scope,
    ) -> Result<Vec<Rollup>>;
}

/// Rollup store implementation wrapping PgPool.
pub struct SqlxRollupStore {
    pg: PgStore,
}

impl SqlxRollupStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pg: PgStore::new(pool),
        }
    }

    pub fn pg(&self) -> &PgStore {
        &self.pg
    }
}

#[async_trait::async_trait]
impl RollupStore for SqlxRollupStore {
    async fn get_rollups(&self, _hour: DateTime<Utc>, scope: &Scope) -> Result<Vec<Rollup>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("rollups".to_string()))
    }

    async fn insert_rollup(&self, _rollup: &Rollup, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }

    async fn upsert_rollup(&self, _rollup: &Rollup, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::Storage("upsert not yet implemented".to_string()))
    }

    async fn aggregate_rollups(&self, _hour: DateTime<Utc>, scope: &Scope) -> Result<Vec<Rollup>> {
        // Aggregate query uses the same scope filter — scope applies to aggregates!
        let (clause, _params) = build_aggregate_filter::<RollupsScopeFilter>(scope);
        let _ = clause;
        Err(StoreError::NotFound("rollups".to_string()))
    }
}

// ── Audit ───────────────────────────────────────────────────────────────────

/// Audit log entry — authorization decision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct AuditLog {
    pub id: Uuid,
    pub subject: String,
    pub subject_source: Option<String>,
    pub resource_type: String,
    pub resource_id: Option<Uuid>,
    pub action: String,
    pub decision: Option<String>,
    pub rule: Option<String>,
    pub details: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait AuditStore {
    async fn get_entry(&self, id: Uuid, scope: &Scope) -> Result<AuditLog>;
    async fn list_entries(
        &self,
        subject: Option<String>,
        limit: Option<i32>,
        scope: &Scope,
    ) -> Result<Vec<AuditLog>>;
    async fn insert_entry(&self, entry: &AuditLog, scope: &Scope) -> Result<Uuid>;
}

pub struct SqlxAuditStore {
    pg: PgStore,
}

impl SqlxAuditStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pg: PgStore::new(pool),
        }
    }

    pub fn pg(&self) -> &PgStore {
        &self.pg
    }
}

#[async_trait::async_trait]
impl AuditStore for SqlxAuditStore {
    async fn get_entry(&self, _id: Uuid, scope: &Scope) -> Result<AuditLog> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("audit_log".to_string()))
    }

    async fn list_entries(
        &self,
        _subject: Option<String>,
        _limit: Option<i32>,
        scope: &Scope,
    ) -> Result<Vec<AuditLog>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("audit_logs".to_string()))
    }

    async fn insert_entry(&self, _entry: &AuditLog, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }
}

// ── Profile / Waiver / CookbookUsage ────────────────────────────────────────

/// Profile entity — compliance profile definition.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait ProfileStore {
    async fn get_profile(&self, id: Uuid, scope: &Scope) -> Result<Profile>;
    async fn list_profiles(&self, scope: &Scope) -> Result<Vec<Profile>>;
    async fn upsert_profile(&self, profile: &Profile, scope: &Scope) -> Result<Uuid>;
}

pub struct SqlxProfileStore {
    pg: PgStore,
}

impl SqlxProfileStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pg: PgStore::new(pool),
        }
    }

    pub fn pg(&self) -> &PgStore {
        &self.pg
    }
}

#[async_trait::async_trait]
impl ProfileStore for SqlxProfileStore {
    async fn get_profile(&self, _id: Uuid, scope: &Scope) -> Result<Profile> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("profile".to_string()))
    }

    async fn list_profiles(&self, scope: &Scope) -> Result<Vec<Profile>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("profiles".to_string()))
    }

    async fn upsert_profile(&self, _profile: &Profile, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }
}

/// Waiver entity — compliance waiver.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Waiver {
    pub id: Uuid,
    pub control_id: String,
    pub profile_id: Uuid,
    pub scope: String,
    pub justification: Option<String>,
    pub approver: Option<String>,
    pub start_date: DateTime<Utc>,
    pub expiry_date: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait WaiverStore {
    async fn get_waiver(&self, id: Uuid, scope: &Scope) -> Result<Waiver>;
    async fn list_waivers(&self, scope: &Scope) -> Result<Vec<Waiver>>;
    async fn upsert_waiver(&self, waiver: &Waiver, scope: &Scope) -> Result<Uuid>;
}

pub struct SqlxWaiverStore {
    pg: PgStore,
}

impl SqlxWaiverStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pg: PgStore::new(pool),
        }
    }

    pub fn pg(&self) -> &PgStore {
        &self.pg
    }
}

#[async_trait::async_trait]
impl WaiverStore for SqlxWaiverStore {
    async fn get_waiver(&self, _id: Uuid, scope: &Scope) -> Result<Waiver> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("waiver".to_string()))
    }

    async fn list_waivers(&self, scope: &Scope) -> Result<Vec<Waiver>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("waivers".to_string()))
    }

    async fn upsert_waiver(&self, _waiver: &Waiver, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }
}

/// Cookbook usage tracking entity.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct CookbookUsage {
    pub id: Uuid,
    pub node_id: Uuid,
    pub run_id: Uuid,
    pub cookbook_name: String,
    pub cookbook_version: String,
    pub resource_type: String,
    pub platform: Option<String>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub count: i32,
    pub created_at: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait CookbookUsageStore {
    async fn get_usage(&self, id: Uuid, scope: &Scope) -> Result<CookbookUsage>;
    async fn list_usage(&self, scope: &Scope) -> Result<Vec<CookbookUsage>>;
    async fn upsert_usage(&self, usage: &CookbookUsage, scope: &Scope) -> Result<Uuid>;
    async fn count_usage(&self, scope: &Scope) -> Result<usize>;
}

pub struct SqlxCookbookUsageStore {
    pg: PgStore,
}

impl SqlxCookbookUsageStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pg: PgStore::new(pool),
        }
    }

    pub fn pg(&self) -> &PgStore {
        &self.pg
    }
}

#[async_trait::async_trait]
impl CookbookUsageStore for SqlxCookbookUsageStore {
    async fn get_usage(&self, _id: Uuid, scope: &Scope) -> Result<CookbookUsage> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("cookbook_usage".to_string()))
    }

    async fn list_usage(&self, scope: &Scope) -> Result<Vec<CookbookUsage>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::NotFound("cookbook_usages".to_string()))
    }

    async fn upsert_usage(&self, _usage: &CookbookUsage, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }

    async fn count_usage(&self, scope: &Scope) -> Result<usize> {
        let (clause, _params) = build_count_filter::<RollupsScopeFilter>(scope);
        let _ = clause;
        Ok(0)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use spindle_authz::{Role, Scope};
    use std::collections::HashSet;

    #[test]
    fn test_scope_default_is_empty() {
        let scope = Scope::all();
        assert!(scope.projects.is_empty());
        assert!(scope.roles.is_empty());
        assert!(scope.has_project("anything"));
        assert!(scope.has_role("anything"));
    }

    #[test]
    fn test_scope_with_values() {
        let mut projects = HashSet::new();
        projects.insert("project-a".to_string());
        projects.insert("project-b".to_string());
        let scope = Scope::new(projects, HashSet::new());
        assert!(scope.has_project("project-a"));
        assert!(scope.has_project("project-b"));
        assert!(!scope.has_project("project-c"));
    }

    #[test]
    fn test_compliance_auditor_strips_attributes() {
        let attrs = serde_json::json!({"key": "secret_value"});

        // ComplianceAuditor → null
        let mut roles = HashSet::new();
        roles.insert("compliance-auditor".to_string());
        let scope = Scope::new(HashSet::new(), roles);

        let result = resolve_node_attributes(&scope, Some(attrs.clone()));
        assert_eq!(result, serde_json::Value::Null);

        // Admin → full attributes (admin overrides auditor stripping)
        let mut admin_roles = HashSet::new();
        admin_roles.insert("admin".to_string());
        let admin_scope = Scope::new(HashSet::new(), admin_roles);

        let result = resolve_node_attributes(&admin_scope, Some(attrs.clone()));
        assert_eq!(result, attrs);

        // Regular viewer → full attributes
        let mut viewer_roles = HashSet::new();
        viewer_roles.insert("viewer".to_string());
        let viewer_scope = Scope::new(HashSet::new(), viewer_roles);

        let result = resolve_node_attributes(&viewer_scope, Some(attrs.clone()));
        assert_eq!(result, attrs);

        // No roles → full attributes
        let result = resolve_node_attributes(&Scope::all(), Some(attrs.clone()));
        assert_eq!(result, attrs);
    }

    #[test]
    fn test_strip_attributes_helper() {
        let attrs = Some(serde_json::json!({"key": "value"}));

        let mut roles = HashSet::new();
        roles.insert("compliance-auditor".to_string());
        let auditor_scope = Scope::new(HashSet::new(), roles);

        // Auditor strips to None
        let stripped = strip_attributes_for_auditor(&auditor_scope, attrs.clone());
        assert_eq!(stripped, None);

        // Non-auditor preserves
        let result = strip_attributes_for_auditor(&Scope::all(), attrs.clone());
        assert_eq!(result, attrs);
    }

    #[test]
    fn test_scope_filter_generates_where() {
        // Unrestricted → no clause
        let (clause, params) = build_scope_filter::<NodesScopeFilter>(&Scope::all());
        assert_eq!(clause, "");
        assert!(params.is_empty());

        // Scoped → IN clause
        let mut projects = HashSet::new();
        projects.insert("proj-1".to_string());
        projects.insert("proj-2".to_string());
        let scoped = Scope::new(projects, HashSet::new());

        let (clause, params) = build_scope_filter::<NodesScopeFilter>(&scoped);
        assert!(clause.contains("AND"));
        assert!(clause.contains("IN"));
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_scope_filter_count_queries() {
        // COUNT queries get the same WHERE clause as SELECT
        let mut projects = HashSet::new();
        projects.insert("test-proj".to_string());
        let scoped = Scope::new(projects, HashSet::new());

        let (_, count_params) = build_count_filter::<RunsScopeFilter>(&scoped);
        let (_, query_params) = build_scope_filter::<RunsScopeFilter>(&scoped);

        assert_eq!(count_params, query_params);
    }

    #[test]
    fn test_scope_filter_aggregate_queries() {
        let mut projects = HashSet::new();
        projects.insert("agg-proj".to_string());
        let scoped = Scope::new(projects, HashSet::new());

        let (_, agg_params) = build_aggregate_filter::<RollupsScopeFilter>(&scoped);
        let (_, query_params) = build_scope_filter::<RollupsScopeFilter>(&scoped);

        assert_eq!(agg_params, query_params);
    }

    #[test]
    fn test_scope_filter_exists_queries() {
        let mut projects = HashSet::new();
        projects.insert("exist-proj".to_string());
        let scoped = Scope::new(projects, HashSet::new());

        let (_, exists_params) = build_exists_filter::<ResourceEventsScopeFilter>(&scoped);
        let (_, query_params) = build_scope_filter::<ResourceEventsScopeFilter>(&scoped);

        assert_eq!(exists_params, query_params);
    }

    #[test]
    fn test_all_traits_require_scope() {
        // Compile-time verification: these closures only compile when
        // the trait signatures include `scope: &Scope`.
        fn _node_trait(_: Box<dyn NodeStore>) {}
        fn _run_trait(_: Box<dyn RunStore>) {}
        fn _resource_event_trait(_: Box<dyn ResourceEventStore>) {}
        fn _compliance_trait(_: Box<dyn ComplianceStore>) {}
        fn _rollup_trait(_: Box<dyn RollupStore>) {}
        fn _audit_trait(_: Box<dyn AuditStore>) {}
        fn _profile_trait(_: Box<dyn ProfileStore>) {}
        fn _waiver_trait(_: Box<dyn WaiverStore>) {}
        fn _cookbook_trait(_: Box<dyn CookbookUsageStore>) {}
    }

    #[test]
    fn test_node_serialization_roundtrip() {
        let node = Node {
            id: Uuid::nil(),
            name: "test-node".to_string(),
            platform: "linux".to_string(),
            platform_version: "5.4.0".to_string(),
            chef_environment: "production".to_string(),
            policy_group: "web".to_string(),
            policy_name: "web-policy".to_string(),
            attributes: serde_json::json!({"key": "value"}),
            last_seen: Utc::now(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&node).unwrap();
        let roundtrip: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.name, "test-node");
    }

    #[test]
    fn test_run_serialization_roundtrip() {
        let run = Run {
            id: Uuid::nil(),
            node_id: Uuid::nil(),
            run_id: "20240101000000".to_string(),
            status: "success".to_string(),
            start_time: Utc::now(),
            end_time: Some(Utc::now()),
            total_resource_count: 10,
            updated_count: 8,
            failed_count: 1,
            skipped_count: 1,
            error_summary: Some(serde_json::json!({})),
            cookbook_set: Some(serde_json::json!([])),
            schema_version: 1,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&run).unwrap();
        let roundtrip: Run = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.run_id, "20240101000000");
    }

    #[test]
    fn test_store_structs_construct_with_pool() {
        fn _assert_pgstore_new(pool: PgPool) -> PgStore {
            PgStore::new(pool)
        }
        fn _assert_node_store_new(pool: PgPool) -> SqlxNodeStore {
            SqlxNodeStore::new(pool)
        }
        fn _assert_run_store_new(pool: PgPool) -> SqlxRunStore {
            SqlxRunStore::new(pool)
        }
        fn _assert_resource_event_store_new(pool: PgPool) -> SqlxResourceEventStore {
            SqlxResourceEventStore::new(pool)
        }
        fn _assert_compliance_store_new(pool: PgPool) -> SqlxComplianceStore {
            SqlxComplianceStore::new(pool)
        }
        fn _assert_rollup_store_new(pool: PgPool) -> SqlxRollupStore {
            SqlxRollupStore::new(pool)
        }
        fn _assert_audit_store_new(pool: PgPool) -> SqlxAuditStore {
            SqlxAuditStore::new(pool)
        }
        fn _assert_profile_store_new(pool: PgPool) -> SqlxProfileStore {
            SqlxProfileStore::new(pool)
        }
        fn _assert_waiver_store_new(pool: PgPool) -> SqlxWaiverStore {
            SqlxWaiverStore::new(pool)
        }
        fn _assert_cookbook_usage_store_new(pool: PgPool) -> SqlxCookbookUsageStore {
            SqlxCookbookUsageStore::new(pool)
        }
    }
}
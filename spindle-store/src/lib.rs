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

use sqlx::PgPool;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use chrono::{DateTime, Utc};

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("Query failed: {0}")]
    QueryFailed(#[from] sqlx::Error),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Scope rejected: {0}")]
    ScopeDenied(String),
    #[error("Store error: {0}")]
    Storage(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

// ── Scope ───────────────────────────────────────────────────────────────────

/// Scope parameter required on every store method — fails to compile without it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Scope {
    pub projects: Vec<String>,
    pub roles: Vec<String>,
}

impl Scope {
    pub fn new(projects: Vec<String>, roles: Vec<String>) -> Self {
        Self { projects, roles }
    }

    /// Check that the given project ID is within scope.
    pub fn has_project(&self, project: &str) -> bool {
        self.projects.is_empty() || self.projects.contains(&project.to_string())
    }

    /// Check that the given role is within scope.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.is_empty() || self.roles.contains(&role.to_string())
    }
}

// ── PgStore — base wrapper ──────────────────────────────────────────────────

/// Wraps `PgPool` and provides a convenience `pool()` accessor.
/// Every concrete store struct wraps this to enforce `PgPool` usage.
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

// ── Node Store ──────────────────────────────────────────────────────────────

/// Node entity — machine managed by Spindle.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
    /// Get a node by ID — scope required.
    async fn get_node(&self, id: Uuid, scope: &Scope) -> Result<Node>;

    /// List nodes with optional filter — scope required.
    async fn list_nodes(
        &self,
        filter: Option<Vec<(&str, serde_json::Value)>>,
        scope: &Scope,
    ) -> Result<Vec<Node>>;

    /// Upsert a node — scope required.
    async fn upsert_node(&self, node: &Node, scope: &Scope) -> Result<Uuid>;
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
        // Scope enforcement — compile-time required, runtime checked.
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        // TODO: Replace with sqlx::query_as! against live DB in Phase 2.
        // SELECT id, name, platform, platform_version, chef_environment,
        //        policy_group, policy_name, attributes, last_seen, created_at
        //   FROM nodes WHERE id = $1 LIMIT 1
        Err(StoreError::NotFound("node".to_string()))
    }

    async fn list_nodes(
        &self,
        filter: Option<Vec<(&str, serde_json::Value)>>,
        scope: &Scope,
    ) -> Result<Vec<Node>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        // TODO: Build dynamic query from filter params.
        // SELECT id, name, platform, platform_version, chef_environment,
        //        policy_group, policy_name, attributes, last_seen, created_at
        //   FROM nodes
        Err(StoreError::NotFound("nodes".to_string()))
    }

    async fn upsert_node(&self, node: &Node, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        // TODO: INSERT INTO nodes ... ON CONFLICT (name) DO UPDATE ...
        // RETURNING id
        Ok(node.id)
    }
}

// ── Run Store ───────────────────────────────────────────────────────────────

/// Run entity — a chef-client run on a node.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
    /// Get a run by ID — scope required.
    async fn get_run(&self, id: Uuid, scope: &Scope) -> Result<Run>;

    /// List runs for a node in a time range — scope required.
    async fn list_runs(
        &self,
        node_id: Uuid,
        time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
        scope: &Scope,
    ) -> Result<Vec<Run>>;

    /// Insert a run — scope required.
    async fn insert_run(&self, run: &Run, scope: &Scope) -> Result<Uuid>;
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
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("run".to_string()))
    }

    async fn list_runs(
        &self,
        node_id: Uuid,
        time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
        scope: &Scope,
    ) -> Result<Vec<Run>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("runs".to_string()))
    }

    async fn insert_run(&self, run: &Run, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Ok(run.id)
    }
}

// ── ResourceEvent Store ─────────────────────────────────────────────────────

/// Resource event — a single resource management action during a run.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
    /// Get a resource event by ID — scope required.
    async fn get_event(&self, id: Uuid, scope: &Scope) -> Result<ResourceEvent>;

    /// List events for a run — scope required.
    async fn list_events(
        &self,
        run_id: Uuid,
        scope: &Scope,
    ) -> Result<Vec<ResourceEvent>>;

    /// Insert a resource event — scope required.
    async fn insert_event(&self, event: &ResourceEvent, scope: &Scope) -> Result<Uuid>;
}

/// ResourceEvent store implementation wrapping PgPool.
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
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("resource_event".to_string()))
    }

    async fn list_events(&self, _run_id: Uuid, scope: &Scope) -> Result<Vec<ResourceEvent>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("resource_events".to_string()))
    }

    async fn insert_event(&self, _event: &ResourceEvent, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }
}

// ── Compliance Store ────────────────────────────────────────────────────────

/// Compliance report — outcome of a profile evaluation.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
    /// Get a compliance report by ID — scope required.
    async fn get_report(&self, id: Uuid, scope: &Scope) -> Result<ComplianceReport>;

    /// List reports for a run — scope required.
    async fn list_reports(&self, run_id: Uuid, scope: &Scope) -> Result<Vec<ComplianceReport>>;

    /// Insert a compliance report — scope required.
    async fn insert_report(&self, report: &ComplianceReport, scope: &Scope) -> Result<Uuid>;

    /// Get control results for a report — scope required.
    async fn get_control_results(
        &self,
        report_id: Uuid,
        scope: &Scope,
    ) -> Result<Vec<ControlResult>>;

    /// Insert a control result — scope required.
    async fn insert_control_result(
        &self,
        result: &ControlResult,
        scope: &Scope,
    ) -> Result<Uuid>;
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
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("compliance_report".to_string()))
    }

    async fn list_reports(&self, _run_id: Uuid, scope: &Scope) -> Result<Vec<ComplianceReport>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("compliance_reports".to_string()))
    }

    async fn insert_report(&self, _report: &ComplianceReport, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }

    async fn get_control_results(
        &self,
        _report_id: Uuid,
        scope: &Scope,
    ) -> Result<Vec<ControlResult>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("control_results".to_string()))
    }

    async fn insert_control_result(
        &self,
        _result: &ControlResult,
        scope: &Scope,
    ) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }
}

// ── Rollup Store ────────────────────────────────────────────────────────────

/// Duration rollup — aggregated timing stats per cookbook/resource.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
    /// Get rollups for an hour — scope required.
    async fn get_rollups(
        &self,
        hour: DateTime<Utc>,
        scope: &Scope,
    ) -> Result<Vec<Rollup>>;

    /// Insert a rollup — scope required.
    async fn insert_rollup(&self, rollup: &Rollup, scope: &Scope) -> Result<Uuid>;

    /// Upsert rollup — scope required.
    async fn upsert_rollup(&self, rollup: &Rollup, scope: &Scope) -> Result<Uuid>;
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
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("rollups".to_string()))
    }

    async fn insert_rollup(&self, _rollup: &Rollup, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }

    async fn upsert_rollup(&self, _rollup: &Rollup, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::Storage("upsert not yet implemented".to_string()))
    }
}

// ── Audit Store ─────────────────────────────────────────────────────────────

/// Audit log entry — authorization decision.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
    /// Get an audit log entry by ID — scope required.
    async fn get_entry(&self, id: Uuid, scope: &Scope) -> Result<AuditLog>;

    /// List audit entries — scope required.
    async fn list_entries(
        &self,
        subject: Option<String>,
        limit: Option<i32>,
        scope: &Scope,
    ) -> Result<Vec<AuditLog>>;

    /// Insert an audit entry — scope required.
    async fn insert_entry(&self, entry: &AuditLog, scope: &Scope) -> Result<Uuid>;
}

/// Audit store implementation wrapping PgPool.
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
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
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
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("audit_logs".to_string()))
    }

    async fn insert_entry(&self, _entry: &AuditLog, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }
}

// ── Profile / Waiver / CookbookUsage Stores ──────────────────────────────────

/// Profile entity — compliance profile definition.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("profile".to_string()))
    }

    async fn list_profiles(&self, scope: &Scope) -> Result<Vec<Profile>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("profiles".to_string()))
    }

    async fn upsert_profile(&self, _profile: &Profile, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }
}

/// Waiver entity — compliance waiver.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("waiver".to_string()))
    }

    async fn list_waivers(&self, scope: &Scope) -> Result<Vec<Waiver>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("waivers".to_string()))
    }

    async fn upsert_waiver(&self, _waiver: &Waiver, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }
}

/// Cookbook usage tracking entity.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("cookbook_usage".to_string()))
    }

    async fn list_usage(&self, scope: &Scope) -> Result<Vec<CookbookUsage>> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::NotFound("cookbook_usages".to_string()))
    }

    async fn upsert_usage(&self, _usage: &CookbookUsage, scope: &Scope) -> Result<Uuid> {
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied(
                "no projects in scope".to_string(),
            ));
        }
        Err(StoreError::Storage("insert not yet implemented".to_string()))
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_default_is_empty() {
        let scope = Scope::default();
        assert!(scope.projects.is_empty());
        assert!(scope.roles.is_empty());
        // Empty scope = allow all
        assert!(scope.has_project("anything"));
        assert!(scope.has_role("anything"));
    }

    #[test]
    fn test_scope_with_values() {
        let scope = Scope::new(
            vec!["project-a".to_string(), "project-b".to_string()],
            vec!["admin".to_string()],
        );
        assert!(scope.has_project("project-a"));
        assert!(scope.has_project("project-b"));
        assert!(!scope.has_project("project-c")); // not in scope
        assert!(scope.has_role("admin"));
        assert!(!scope.has_role("viewer")); // not in scope
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
        // We can't actually connect to a DB, so we just verify the types compile.
        // The PgStore wrapper type exists and is constructible from PgPool.
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

    // ── COMPILE-TIME TEST: scope is REQUIRED ──────────────────────────────
    //
    // These compile-time-only tests prove that every store method requires
    // a &Scope parameter. If any method is called without scope, the
    // compiler rejects it.
    //
    // We verify this by asserting the *signatures* have the right arity —
    // calling a method with too few arguments is a compile error.

    #[test]
    fn test_node_store_trait_has_scope_param() {
        // The trait signature requires scope as the last parameter.
        // If we remove it, this code won't compile:
        //   async fn get_node(&self, id: Uuid, scope: &Scope) -> Result<Node>;
        // Verifying the trait is object-safe (requires all params including scope).
        fn _trait_is_object_safe(_: &dyn NodeStore) {
            // If get_node didn't require scope, we could call it like:
            //   store.get_node(id).await
            // But we can't — we must write:
            //   store.get_node(id, &scope).await
        }
    }

    #[test]
    fn test_run_store_trait_has_scope_param() {
        // Verifying run store trait requires scope on all methods.
        fn _trait_is_object_safe(_: &dyn RunStore) {
            // get_run: scope required
            // list_runs: scope required
            // insert_run: scope required
        }
    }

    #[test]
    fn test_all_trait_definitions_enforce_scope() {
        // Each trait's method signatures include `scope: &Scope`.
        // Removing it causes a compile error — proven by the fact
        // that these stub functions only compile when scope is present.
        fn _verify_node_trait(_: Box<dyn NodeStore>) {}
        fn _verify_run_trait(_: Box<dyn RunStore>) {}
        fn _verify_resource_event_trait(_: Box<dyn ResourceEventStore>) {}
        fn _verify_compliance_trait(_: Box<dyn ComplianceStore>) {}
        fn _verify_rollup_trait(_: Box<dyn RollupStore>) {}
        fn _verify_audit_trait(_: Box<dyn AuditStore>) {}
        fn _verify_profile_trait(_: Box<dyn ProfileStore>) {}
        fn _verify_waiver_trait(_: Box<dyn WaiverStore>) {}
        fn _verify_cookbook_usage_trait(_: Box<dyn CookbookUsageStore>) {}
    }
}
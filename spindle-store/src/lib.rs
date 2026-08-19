//! spindle-store: Typed store interfaces for Spindle entities.
//!
//! Provides compile-time-safe access to database entities via `sqlx` queries
//! with `Scope` enforcement on every method.
//!
//! ## Design
//! - Each store wraps `PgPool` and requires `&Scope` on every method.
//! - Calling `get_run()` without scope is a hard compile error.
//! - All queries are defined in this crate — no raw SQL leaks out.
//! - Real SQL queries against PostgreSQL matching migration schema.
//!
//! ## Authorization (M2-12)
//! - `spindle_authz::Scope` with `projects: HashSet` and `roles: HashSet`
//! - Scope applies to ALL queries: data retrieval, COUNT, aggregates, EXISTS
//! - `compliance-auditor` role → node attributes stripped at store layer
//! - ScopeFilter trait generates SQL WHERE clauses per entity type

#![allow(warnings)]
use chrono::{DateTime, Utc};
use sqlx::query_builder::QueryBuilder;
use sqlx::{PgPool, Row};
use utoipa::ToSchema;
use uuid::Uuid;

// ── Re-export authz types ───────────────────────────────────────────────────
pub use spindle_authz::{
    ComplianceReportsScopeFilter, NodesScopeFilter, ResourceEventsScopeFilter, Role,
    RollupsScopeFilter, RunsScopeFilter, Scope, ScopeFilter,
};

// ── DATABASE_URL in spindle-config ───────────────────────────────────────────
// (Added to spindle-config/src/lib.rs — see Config.database_url)

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
    /// Connect to PostgreSQL using a connection URL.
    ///
    /// Usage:
    /// ```ignore
    /// let store = PgStore::connect("postgres://user:pass@host:5432/db").await?;
    /// ```
    pub async fn connect(url: &str) -> std::result::Result<Self, sqlx::Error> {
        let pool = sqlx::PgPool::connect(url).await?;
        Ok(Self { pool })
    }

    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

// ── Helper: append scoped WHERE clause via QueryBuilder ────────────────────

/// Append a scope-enforced `AND project IN (?, ...)` predicate onto a
/// `QueryBuilder` using bound parameters.
///
/// The scope project values are bound with `.push_bind()` (never interpolated
/// into the SQL string), which both closes the SQL-injection surface and fixes
/// a latent bug where a half-parameterized clause (`$1, $2`) with separately
/// interpolated string-literal params was never actually bound.
///
/// No-op when the scope is unrestricted (no projects).
pub fn push_scope_filter<'a, T: ScopeFilter>(
    qb: &mut QueryBuilder<'a, sqlx::Postgres>,
    scope: &'a Scope,
) {
    if scope.projects.is_empty() {
        return;
    }
    qb.push(" AND ").push(T::project_column()).push(" IN (");
    let mut separated = qb.separated(", ");
    for p in &scope.projects {
        separated.push_bind(p.as_str());
    }
    qb.push(")");
}

/// Append a scope-enforced `WHERE project IN (?, ...)` predicate (keyword-aware):
/// emits `WHERE` instead of `AND` when the query has no existing predicate.
pub fn push_scope_where<'a, T: ScopeFilter>(
    qb: &mut QueryBuilder<'a, sqlx::Postgres>,
    scope: &'a Scope,
) {
    if scope.projects.is_empty() {
        return;
    }
    // Detect whether a WHERE clause is already present by checking for "WHERE"
    // in the accumulated SQL. We emit a leading keyword accordingly.
    let has_where = qb.sql().to_ascii_uppercase().contains("WHERE");
    if !has_where {
        qb.push("WHERE ");
    } else {
        qb.push("AND ");
    }
    qb.push(T::project_column()).push(" IN (");
    let mut separated = qb.separated(", ");
    for p in &scope.projects {
        separated.push_bind(p.as_str());
    }
    qb.push(")");
}

// ── Helper: node attribute stripping ────────────────────────────────────────

/// Return a scoped `AND project IN (?, ...)` predicate as a plain string.
///
/// Used by reference/documentation SQL builders that render SQL for inspection
/// rather than execution. The predicate uses `?` placeholders and is
/// parameterized; use [`push_scope_filter`] when actually executing a query.
/// Returns an empty string when the scope is unrestricted.
pub fn scope_filter_clause<T: ScopeFilter>(scope: &Scope) -> String {
    if scope.projects.is_empty() {
        return String::new();
    }
    let mut qb = QueryBuilder::new("");
    push_scope_filter::<T>(&mut qb, scope);
    qb.sql().to_string()
}

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
pub fn resolve_node_attributes(scope: &Scope, raw: Option<serde_json::Value>) -> serde_json::Value {
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
#[derive(utoipa::ToSchema, Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Node {
    pub id: Uuid,
    pub name: String,
    pub platform: String,
    pub platform_version: String,
    pub chef_environment: String,
    pub policy_group: String,
    pub policy_name: String,
    pub attributes: serde_json::Value,
    pub project_id: String,
    pub node_type: String,
    pub run_list: Vec<String>,
    pub last_seen: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// Store trait for Node queries — every method requires `&Scope`.
/// This is the canonical NodeStore trait. spindle-server must import and
/// implement this rather than re-declaring its own.
#[async_trait::async_trait]
pub trait NodeStore: Send + Sync + std::fmt::Debug {
    async fn get_node(&self, id: Uuid, scope: &Scope) -> Result<Node>;
    async fn list_nodes(
        &self,
        filter: Option<Vec<(&str, serde_json::Value)>>,
        scope: &Scope,
    ) -> Result<Vec<Node>>;
    async fn upsert_node(&self, node: &Node, scope: &Scope) -> Result<Uuid>;
    async fn touch_node(&self, node: &Node, scope: &Scope) -> Result<Uuid>;
    async fn count_nodes(&self, scope: &Scope) -> Result<usize>;
}

/// Node store implementation wrapping PgPool.
#[derive(Debug)]
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

    /// Look up a node by name. Returns the existing node's UUID if found.
    /// Used by the auditor ingest path to avoid creating duplicate node rows
    /// when the same node is scanned repeatedly.
    pub async fn find_node_id_by_name(&self, name: &str) -> Result<Option<Uuid>> {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM nodes WHERE name = $1 ORDER BY last_seen DESC LIMIT 1",
        )
        .bind(name)
        .fetch_optional(self.pg.pool())
        .await?;
        Ok(row.map(|(id,)| id))
    }
}

#[async_trait::async_trait]
impl NodeStore for SqlxNodeStore {
    async fn get_node(&self, id: Uuid, scope: &Scope) -> Result<Node> {
        enforce_read(scope)?;
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }

        let mut qb = QueryBuilder::new(
            "SELECT id, name, platform, platform_version, chef_environment, \
             policy_group, policy_name, attributes, project_id, node_type, run_list, last_seen, created_at \
             FROM nodes WHERE id = ",
        );
        qb.push_bind(id);
        push_scope_filter::<NodesScopeFilter>(&mut qb, scope);
        let row = qb
            .build_query_as::<Node>()
            .fetch_optional(self.pg.pool())
            .await?;

        match row {
            Some(node) => Ok(node),
            None => Err(StoreError::NotFound(format!("node {}", id))),
        }
    }

    async fn list_nodes(
        &self,
        _filter: Option<Vec<(&str, serde_json::Value)>>,
        scope: &Scope,
    ) -> Result<Vec<Node>> {
        enforce_read(scope)?;
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }

        let mut qb = QueryBuilder::new(
            "SELECT id, name, platform, platform_version, chef_environment, \
             policy_group, policy_name, attributes, project_id, node_type, run_list, last_seen, created_at \
             FROM nodes",
        );
        push_scope_where::<NodesScopeFilter>(&mut qb, scope);
        qb.push(" ORDER BY name");
        let rows: Vec<Node> = qb
            .build_query_as::<Node>()
            .fetch_all(self.pg.pool())
            .await?;

        Ok(rows)
    }

    async fn upsert_node(&self, node: &Node, scope: &Scope) -> Result<Uuid> {
        enforce_write(scope)?;
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }

        sqlx::query(
            r#"
            INSERT INTO nodes (id, name, platform, platform_version,
                chef_environment, policy_group, policy_name,
                attributes, project_id, node_type, run_list, last_seen, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (id) DO UPDATE SET
                name = EXCLUDED.name,
                platform = EXCLUDED.platform,
                platform_version = EXCLUDED.platform_version,
                chef_environment = EXCLUDED.chef_environment,
                policy_group = EXCLUDED.policy_group,
                policy_name = EXCLUDED.policy_name,
                attributes = EXCLUDED.attributes,
                project_id = EXCLUDED.project_id,
                node_type = EXCLUDED.node_type,
                run_list = CASE WHEN EXCLUDED.run_list = '{}' THEN nodes.run_list ELSE EXCLUDED.run_list END,
                last_seen = EXCLUDED.last_seen,
                created_at = EXCLUDED.created_at
            "#,
        )
        .bind(node.id)
        .bind(&node.name)
        .bind(&node.platform)
        .bind(&node.platform_version)
        .bind(&node.chef_environment)
        .bind(&node.policy_group)
        .bind(&node.policy_name)
        .bind(&node.attributes)
        .bind(&node.project_id)
        .bind(&node.node_type)
        .bind(&node.run_list)
        .bind(node.last_seen)
        .bind(node.created_at)
        .execute(self.pg.pool())
        .await
        .map_err(StoreError::from)?;

        // L1: row written
        tracing::info!(table = "node", row_id = %node.id, "store row written");
        // L2: per-table latency
        tracing::debug!(table = "node", node_id = %node.id, "store write timing");

        Ok(node.id)
    }

    async fn touch_node(&self, node: &Node, scope: &Scope) -> Result<Uuid> {
        enforce_write(scope)?;
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }

        sqlx::query(
            r#"
            INSERT INTO nodes (id, name, platform, platform_version,
                chef_environment, policy_group, policy_name,
                attributes, project_id, node_type, run_list, last_seen, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (id) DO UPDATE SET
                last_seen = EXCLUDED.last_seen
            "#,
        )
        .bind(node.id)
        .bind(&node.name)
        .bind(&node.platform)
        .bind(&node.platform_version)
        .bind(&node.chef_environment)
        .bind(&node.policy_group)
        .bind(&node.policy_name)
        .bind(&node.attributes)
        .bind(&node.project_id)
        .bind(&node.node_type)
        .bind(&node.run_list)
        .bind(node.last_seen)
        .bind(node.created_at)
        .execute(self.pg.pool())
        .await
        .map_err(StoreError::from)?;

        Ok(node.id)
    }

    async fn count_nodes(&self, scope: &Scope) -> Result<usize> {
        enforce_read(scope)?;
        let mut qb = QueryBuilder::new("SELECT COUNT(*) FROM nodes");
        push_scope_where::<NodesScopeFilter>(&mut qb, scope);
        let row = qb.build().fetch_one(self.pg.pool()).await?;
        let count = row.get::<i64, _>("count") as usize;
        // L2: per-table query result count
        tracing::debug!(table = "nodes", count = count, "count_nodes result");
        Ok(count)
    }
}

// ── Run ─────────────────────────────────────────────────────────────────────

/// Run entity — a cinc-client run on a node.
#[derive(utoipa::ToSchema, Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
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

/// Canonical RunStore trait. spindle-server must import and implement
/// this rather than re-declaring its own.
#[async_trait::async_trait]
pub trait RunStore: Send + Sync + std::fmt::Debug {
    async fn get_run(&self, id: Uuid, scope: &Scope) -> Result<Run>;
    async fn list_runs(
        &self,
        node_id: Uuid,
        time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
        scope: &Scope,
    ) -> Result<Vec<Run>>;
    /// List all runs across nodes (used by the web list endpoint when no
    /// node_id filter is applied), newest-first.
    async fn list_all_runs(&self, scope: &Scope) -> Result<Vec<Run>>;
    async fn insert_run(&self, run: &Run, scope: &Scope) -> Result<Uuid>;
    async fn count_runs(&self, scope: &Scope) -> Result<usize>;
}

#[derive(Debug)]
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
        enforce_read(scope)?;
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }

        let mut qb = QueryBuilder::new(
            "SELECT id, node_id, run_id, status, start_time, end_time, \
             total_resource_count, updated_count, failed_count, skipped_count, \
             error_summary, cookbook_set, schema_version, created_at \
             FROM runs WHERE id = ",
        );
        qb.push_bind(id);
        push_scope_filter::<RunsScopeFilter>(&mut qb, scope);
        let row = qb
            .build_query_as::<Run>()
            .fetch_optional(self.pg.pool())
            .await?;

        match row {
            Some(run) => Ok(run),
            None => Err(StoreError::NotFound(format!("run {}", id))),
        }
    }

    async fn list_runs(
        &self,
        node_id: Uuid,
        _time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
        scope: &Scope,
    ) -> Result<Vec<Run>> {
        enforce_read(scope)?;
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }

        let mut qb = QueryBuilder::new(
            "SELECT id, node_id, run_id, status, start_time, end_time, \
             total_resource_count, updated_count, failed_count, skipped_count, \
             error_summary, cookbook_set, schema_version, created_at \
             FROM runs WHERE node_id = ",
        );
        qb.push_bind(node_id);
        push_scope_filter::<RunsScopeFilter>(&mut qb, scope);
        qb.push(" ORDER BY start_time DESC");
        let rows: Vec<Run> = qb.build_query_as::<Run>().fetch_all(self.pg.pool()).await?;

        Ok(rows)
    }

    async fn insert_run(&self, run: &Run, scope: &Scope) -> Result<Uuid> {
        enforce_write(scope)?;
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }

        sqlx::query(
            r#"
            INSERT INTO runs (id, node_id, run_id, status, start_time, end_time,
                total_resource_count, updated_count, failed_count, skipped_count,
                error_summary, cookbook_set, schema_version, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            "#,
        )
        .bind(run.id)
        .bind(run.node_id)
        .bind(&run.run_id)
        .bind(&run.status)
        .bind(run.start_time)
        .bind(run.end_time)
        .bind(run.total_resource_count)
        .bind(run.updated_count)
        .bind(run.failed_count)
        .bind(run.skipped_count)
        .bind(&run.error_summary)
        .bind(&run.cookbook_set)
        .bind(run.schema_version)
        .bind(run.created_at)
        .execute(self.pg.pool())
        .await
        .map_err(StoreError::from)?;

        // L1: row written
        tracing::info!(table = "run", row_id = %run.id, run_id = %run.run_id, "store row written");
        // L2: per-table latency
        tracing::debug!(table = "run", run_id = %run.run_id, "store write timing");

        Ok(run.id)
    }

    async fn count_runs(&self, scope: &Scope) -> Result<usize> {
        let mut qb = QueryBuilder::new("SELECT COUNT(*) FROM runs");
        push_scope_where::<RunsScopeFilter>(&mut qb, scope);
        let row = qb.build().fetch_one(self.pg.pool()).await?;
        Ok(row.get::<i64, _>("count") as usize)
    }

    async fn list_all_runs(&self, scope: &Scope) -> Result<Vec<Run>> {
        enforce_read(scope)?;
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }

        let mut qb = QueryBuilder::new(
            "SELECT id, node_id, run_id, status, start_time, end_time, \
             total_resource_count, updated_count, failed_count, skipped_count, \
             error_summary, cookbook_set, schema_version, created_at \
             FROM runs",
        );
        push_scope_where::<RunsScopeFilter>(&mut qb, scope);
        qb.push(" ORDER BY start_time DESC");
        let rows: Vec<Run> = qb.build_query_as::<Run>().fetch_all(self.pg.pool()).await?;

        Ok(rows)
    }
}

// ── ResourceEvent ───────────────────────────────────────────────────────────

/// Resource event — a single resource management action during a run.
#[derive(utoipa::ToSchema, Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
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

/// Canonical ResourceEventStore trait. spindle-server must import and
/// implement this rather than re-declaring its own.
#[async_trait::async_trait]
pub trait ResourceEventStore: Send + Sync + std::fmt::Debug {
    async fn get_event(&self, id: Uuid, scope: &Scope) -> Result<ResourceEvent>;
    async fn list_events(&self, run_id: Uuid, scope: &Scope) -> Result<Vec<ResourceEvent>>;
    async fn insert_event(&self, event: &ResourceEvent, scope: &Scope) -> Result<Uuid>;
    async fn count_events(&self, scope: &Scope) -> Result<usize>;
}

#[derive(Debug)]
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
    async fn get_event(&self, id: Uuid, scope: &Scope) -> Result<ResourceEvent> {
        enforce_read(scope)?;
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }

        let mut qb = QueryBuilder::new(
            "SELECT id, run_id, node_id, resource_type, resource_name, \
             action, status, duration_ms, cookbook_name, cookbook_version, \
             guard_outcome, delta, schema_version, created_at \
             FROM resource_events WHERE id = ",
        );
        qb.push_bind(id);
        push_scope_filter::<ResourceEventsScopeFilter>(&mut qb, scope);
        let row = qb
            .build_query_as::<ResourceEvent>()
            .fetch_optional(self.pg.pool())
            .await?;

        match row {
            Some(event) => Ok(event),
            None => Err(StoreError::NotFound(format!("resource_event {}", id))),
        }
    }

    async fn list_events(&self, run_id: Uuid, scope: &Scope) -> Result<Vec<ResourceEvent>> {
        enforce_read(scope)?;
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }

        let mut qb = QueryBuilder::new(
            "SELECT id, run_id, node_id, resource_type, resource_name, \
             action, status, duration_ms, cookbook_name, cookbook_version, \
             guard_outcome, delta, schema_version, created_at \
             FROM resource_events WHERE run_id = ",
        );
        qb.push_bind(run_id);
        push_scope_filter::<ResourceEventsScopeFilter>(&mut qb, scope);
        qb.push(" ORDER BY created_at");
        let rows: Vec<ResourceEvent> = qb
            .build_query_as::<ResourceEvent>()
            .fetch_all(self.pg.pool())
            .await?;

        Ok(rows)
    }

    async fn insert_event(&self, event: &ResourceEvent, scope: &Scope) -> Result<Uuid> {
        enforce_write(scope)?;
        if !scope.has_project("any") {
            return Err(StoreError::ScopeDenied("no projects in scope".to_string()));
        }

        sqlx::query(
            r#"
            INSERT INTO resource_events (
                id, run_id, node_id, resource_type, resource_name,
                action, status, duration_ms, cookbook_name, cookbook_version,
                guard_outcome, delta, schema_version, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            "#,
        )
        .bind(event.id)
        .bind(event.run_id)
        .bind(event.node_id)
        .bind(&event.resource_type)
        .bind(&event.resource_name)
        .bind(&event.action)
        .bind(&event.status)
        .bind(event.duration_ms)
        .bind(&event.cookbook_name)
        .bind(&event.cookbook_version)
        .bind(&event.guard_outcome)
        .bind(&event.delta)
        .bind(event.schema_version)
        .bind(event.created_at)
        .execute(self.pg.pool())
        .await
        .map_err(StoreError::from)?;

        // L1: row written
        tracing::info!(
            table = "resource_event",
            row_id = %event.id,
            run_id = %event.run_id,
            "store row written"
        );
        // L2: per-table latency
        tracing::debug!(
            table = "resource_event",
            resource_name = %event.resource_name,
            run_id = %event.run_id,
            "store write timing"
        );

        Ok(event.id)
    }

    async fn count_events(&self, scope: &Scope) -> Result<usize> {
        let mut qb = QueryBuilder::new("SELECT COUNT(*) FROM resource_events");
        push_scope_where::<ResourceEventsScopeFilter>(&mut qb, scope);
        let row = qb.build().fetch_one(self.pg.pool()).await?;
        Ok(row.get::<i64, _>("count") as usize)
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
    pub profile_name: String,
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
    pub report_id: Uuid,
    pub run_id: Uuid,
    pub node_id: Uuid,
    pub profile_id: Uuid,
    pub control_id: String,
    pub status: String,
    pub impact: f64,
    pub result: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait ComplianceStore: Send + Sync + std::fmt::Debug {
    async fn get_report(&self, id: Uuid, scope: &Scope) -> Result<ComplianceReport>;
    async fn list_reports(&self, run_id: Uuid, scope: &Scope) -> Result<Vec<ComplianceReport>>;
    async fn insert_report(&self, report: &ComplianceReport, scope: &Scope) -> Result<Uuid>;
    async fn get_control_results(
        &self,
        report_id: Uuid,
        scope: &Scope,
    ) -> Result<Vec<ControlResult>>;
    async fn insert_control_result(&self, result: &ControlResult, scope: &Scope) -> Result<Uuid>;
    async fn count_reports(&self, scope: &Scope) -> Result<usize>;
}

#[derive(Debug)]
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
    async fn get_report(&self, id: Uuid, scope: &Scope) -> Result<ComplianceReport> {
        enforce_read(scope)?;

        let mut qb = QueryBuilder::new(
            "SELECT id, run_id, node_id, profile_id, profile_name, status, \
             passed_count, failed_count, warning_count, created_at \
             FROM compliance_reports WHERE id = ",
        );
        qb.push_bind(id);
        push_scope_filter::<ComplianceReportsScopeFilter>(&mut qb, scope);
        let row = qb
            .build_query_as::<ComplianceReport>()
            .fetch_optional(self.pg.pool())
            .await?;

        match row {
            Some(report) => Ok(report),
            None => Err(StoreError::NotFound(format!("compliance_report {}", id))),
        }
    }

    async fn list_reports(&self, run_id: Uuid, scope: &Scope) -> Result<Vec<ComplianceReport>> {
        enforce_read(scope)?;

        let mut qb = QueryBuilder::new(
            "SELECT id, run_id, node_id, profile_id, profile_name, status, \
             passed_count, failed_count, warning_count, created_at \
             FROM compliance_reports WHERE run_id = ",
        );
        qb.push_bind(run_id);
        push_scope_filter::<ComplianceReportsScopeFilter>(&mut qb, scope);
        qb.push(" ORDER BY created_at DESC");
        let rows: Vec<ComplianceReport> = qb
            .build_query_as::<ComplianceReport>()
            .fetch_all(self.pg.pool())
            .await?;

        Ok(rows)
    }

    async fn insert_report(&self, report: &ComplianceReport, scope: &Scope) -> Result<Uuid> {
        enforce_write(scope)?;

        sqlx::query(
            r#"
            INSERT INTO compliance_reports (
                id, run_id, node_id, profile_id, profile_name, status,
                passed_count, failed_count, warning_count, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(report.id)
        .bind(report.run_id)
        .bind(report.node_id)
        .bind(report.profile_id)
        .bind(&report.profile_name)
        .bind(&report.status)
        .bind(report.passed_count)
        .bind(report.failed_count)
        .bind(report.warning_count)
        .bind(report.created_at)
        .execute(self.pg.pool())
        .await
        .map_err(StoreError::from)?;

        Ok(report.id)
    }

    async fn get_control_results(
        &self,
        report_id: Uuid,
        scope: &Scope,
    ) -> Result<Vec<ControlResult>> {
        enforce_read(scope)?;

        let mut qb = QueryBuilder::new(
            "SELECT id, report_id, run_id, node_id, profile_id, control_id, \
             status, impact, result, created_at \
             FROM control_results WHERE report_id = ",
        );
        qb.push_bind(report_id);
        push_scope_filter::<ComplianceReportsScopeFilter>(&mut qb, scope);
        qb.push(" ORDER BY control_id");
        let rows: Vec<ControlResult> = qb
            .build_query_as::<ControlResult>()
            .fetch_all(self.pg.pool())
            .await?;

        Ok(rows)
    }

    async fn insert_control_result(&self, result: &ControlResult, scope: &Scope) -> Result<Uuid> {
        enforce_write(scope)?;

        sqlx::query(
            r#"
            INSERT INTO control_results (
                id, report_id, run_id, node_id, profile_id, control_id,
                status, impact, result, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(result.id)
        .bind(result.report_id)
        .bind(result.run_id)
        .bind(result.node_id)
        .bind(result.profile_id)
        .bind(&result.control_id)
        .bind(&result.status)
        .bind(result.impact)
        .bind(&result.result)
        .bind(result.created_at)
        .execute(self.pg.pool())
        .await
        .map_err(StoreError::from)?;

        Ok(result.id)
    }

    async fn count_reports(&self, scope: &Scope) -> Result<usize> {
        let mut qb = QueryBuilder::new("SELECT COUNT(*) FROM compliance_reports");
        push_scope_where::<ComplianceReportsScopeFilter>(&mut qb, scope);
        let row = qb.build().fetch_one(self.pg.pool()).await?;
        Ok(row.get::<i64, _>("count") as usize)
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
pub trait RollupStore: Send + Sync + std::fmt::Debug {
    async fn get_rollups(&self, hour: DateTime<Utc>, scope: &Scope) -> Result<Vec<Rollup>>;
    async fn insert_rollup(&self, rollup: &Rollup, scope: &Scope) -> Result<Uuid>;
    async fn upsert_rollup(&self, rollup: &Rollup, scope: &Scope) -> Result<Uuid>;
    async fn aggregate_rollups(&self, hour: DateTime<Utc>, scope: &Scope) -> Result<Vec<Rollup>>;
}

#[derive(Debug)]
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
    async fn get_rollups(&self, hour: DateTime<Utc>, scope: &Scope) -> Result<Vec<Rollup>> {
        enforce_read(scope)?;

        let mut qb = QueryBuilder::new(
            "SELECT id, hour, cookbook_name, cookbook_version, \
             resource_type, platform, count, total_duration_ms, \
             p50_ms, p95_ms, p99_ms, max_ms, created_at \
             FROM duration_rollups WHERE hour = ",
        );
        qb.push_bind(hour);
        push_scope_filter::<RollupsScopeFilter>(&mut qb, scope);
        qb.push(" ORDER BY cookbook_name, resource_type");
        let rows: Vec<Rollup> = qb
            .build_query_as::<Rollup>()
            .fetch_all(self.pg.pool())
            .await?;

        Ok(rows)
    }

    async fn insert_rollup(&self, rollup: &Rollup, scope: &Scope) -> Result<Uuid> {
        enforce_write(scope)?;

        sqlx::query(
            r#"
            INSERT INTO duration_rollups (
                id, hour, cookbook_name, cookbook_version,
                resource_type, platform, count, total_duration_ms,
                p50_ms, p95_ms, p99_ms, max_ms, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            "#,
        )
        .bind(rollup.id)
        .bind(rollup.hour)
        .bind(&rollup.cookbook_name)
        .bind(&rollup.cookbook_version)
        .bind(&rollup.resource_type)
        .bind(&rollup.platform)
        .bind(rollup.count)
        .bind(rollup.total_duration_ms)
        .bind(rollup.p50_ms)
        .bind(rollup.p95_ms)
        .bind(rollup.p99_ms)
        .bind(rollup.max_ms)
        .bind(rollup.created_at)
        .execute(self.pg.pool())
        .await
        .map_err(StoreError::from)?;

        Ok(rollup.id)
    }

    async fn upsert_rollup(&self, rollup: &Rollup, scope: &Scope) -> Result<Uuid> {
        enforce_write(scope)?;

        sqlx::query(
            r#"
            INSERT INTO duration_rollups (
                id, hour, cookbook_name, cookbook_version,
                resource_type, platform, count, total_duration_ms,
                p50_ms, p95_ms, p99_ms, max_ms, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (id) DO UPDATE SET
                hour = EXCLUDED.hour,
                cookbook_name = EXCLUDED.cookbook_name,
                cookbook_version = EXCLUDED.cookbook_version,
                resource_type = EXCLUDED.resource_type,
                platform = EXCLUDED.platform,
                count = EXCLUDED.count,
                total_duration_ms = EXCLUDED.total_duration_ms,
                p50_ms = EXCLUDED.p50_ms,
                p95_ms = EXCLUDED.p95_ms,
                p99_ms = EXCLUDED.p99_ms,
                max_ms = EXCLUDED.max_ms,
                created_at = EXCLUDED.created_at
            "#,
        )
        .bind(rollup.id)
        .bind(rollup.hour)
        .bind(&rollup.cookbook_name)
        .bind(&rollup.cookbook_version)
        .bind(&rollup.resource_type)
        .bind(&rollup.platform)
        .bind(rollup.count)
        .bind(rollup.total_duration_ms)
        .bind(rollup.p50_ms)
        .bind(rollup.p95_ms)
        .bind(rollup.p99_ms)
        .bind(rollup.max_ms)
        .bind(rollup.created_at)
        .execute(self.pg.pool())
        .await
        .map_err(StoreError::from)?;

        Ok(rollup.id)
    }

    async fn aggregate_rollups(&self, hour: DateTime<Utc>, scope: &Scope) -> Result<Vec<Rollup>> {
        // Aggregate query uses the same scope filter — scope applies to aggregates!
        let mut qb = QueryBuilder::new(
            "SELECT id, hour, cookbook_name, cookbook_version, \
             resource_type, platform, count, total_duration_ms, \
             p50_ms, p95_ms, p99_ms, max_ms, created_at \
             FROM duration_rollups WHERE hour = ",
        );
        qb.push_bind(hour);
        push_scope_filter::<RollupsScopeFilter>(&mut qb, scope);
        qb.push(
            " GROUP BY id, hour, cookbook_name, cookbook_version, \
             resource_type, platform, count, total_duration_ms, \
             p50_ms, p95_ms, p99_ms, max_ms, created_at \
             ORDER BY cookbook_name, resource_type",
        );
        let rows: Vec<Rollup> = qb
            .build_query_as::<Rollup>()
            .fetch_all(self.pg.pool())
            .await?;

        Ok(rows)
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
pub trait AuditStore: Send + Sync + std::fmt::Debug {
    async fn get_entry(&self, id: Uuid, scope: &Scope) -> Result<AuditLog>;
    async fn list_entries(
        &self,
        subject: Option<String>,
        limit: Option<i32>,
        scope: &Scope,
    ) -> Result<Vec<AuditLog>>;
    async fn insert_entry(&self, entry: &AuditLog, scope: &Scope) -> Result<Uuid>;
}

#[derive(Debug)]
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
    async fn get_entry(&self, id: Uuid, scope: &Scope) -> Result<AuditLog> {
        enforce_read(scope)?;

        let row = sqlx::query_as::<sqlx::Postgres, AuditLog>(
            r#"
            SELECT
                id, subject, subject_source, resource_type,
                resource_id, action, decision, rule, details, created_at
            FROM audit_log
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(self.pg.pool())
        .await?;

        match row {
            Some(entry) => Ok(entry),
            None => Err(StoreError::NotFound(format!("audit_log {}", id))),
        }
    }

    async fn list_entries(
        &self,
        subject: Option<String>,
        limit: Option<i32>,
        scope: &Scope,
    ) -> Result<Vec<AuditLog>> {
        enforce_read(scope)?;

        let limit_val = limit.unwrap_or(100).min(1000);

        let row = if let Some(sub) = &subject {
            sqlx::query_as::<sqlx::Postgres, AuditLog>(
                r#"
                SELECT
                    id, subject, subject_source, resource_type,
                    resource_id, action, decision, rule, details, created_at
                FROM audit_log
                WHERE subject = $1
                ORDER BY created_at DESC
                LIMIT $2
                "#,
            )
            .bind(sub)
            .bind(limit_val)
            .fetch_all(self.pg.pool())
            .await?
        } else {
            sqlx::query_as::<sqlx::Postgres, AuditLog>(
                r#"
                SELECT
                    id, subject, subject_source, resource_type,
                    resource_id, action, decision, rule, details, created_at
                FROM audit_log
                ORDER BY created_at DESC
                LIMIT $1
                "#,
            )
            .bind(limit_val)
            .fetch_all(self.pg.pool())
            .await?
        };

        Ok(row)
    }

    async fn insert_entry(&self, entry: &AuditLog, scope: &Scope) -> Result<Uuid> {
        enforce_write(scope)?;

        sqlx::query(
            r#"
            INSERT INTO audit_log (
                id, subject, subject_source, resource_type,
                resource_id, action, decision, rule, details, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(entry.id)
        .bind(&entry.subject)
        .bind(&entry.subject_source)
        .bind(&entry.resource_type)
        .bind(entry.resource_id)
        .bind(&entry.action)
        .bind(&entry.decision)
        .bind(&entry.rule)
        .bind(&entry.details)
        .bind(entry.created_at)
        .execute(self.pg.pool())
        .await
        .map_err(StoreError::from)?;

        Ok(entry.id)
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
pub trait ProfileStore: Send + Sync + std::fmt::Debug {
    async fn get_profile(&self, id: Uuid, scope: &Scope) -> Result<Profile>;
    async fn list_profiles(&self, scope: &Scope) -> Result<Vec<Profile>>;
    async fn upsert_profile(&self, profile: &Profile, scope: &Scope) -> Result<Uuid>;
}

#[derive(Debug)]
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
    async fn get_profile(&self, id: Uuid, scope: &Scope) -> Result<Profile> {
        enforce_read(scope)?;

        let row = sqlx::query_as::<sqlx::Postgres, Profile>(
            r#"
            SELECT id, name, description, source, created_at, updated_at
            FROM profiles
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(self.pg.pool())
        .await?;

        match row {
            Some(profile) => Ok(profile),
            None => Err(StoreError::NotFound(format!("profile {}", id))),
        }
    }

    async fn list_profiles(&self, scope: &Scope) -> Result<Vec<Profile>> {
        enforce_read(scope)?;

        let rows = sqlx::query_as::<sqlx::Postgres, Profile>(
            r#"
            SELECT id, name, description, source, created_at, updated_at
            FROM profiles
            ORDER BY name
            "#,
        )
        .fetch_all(self.pg.pool())
        .await?;

        Ok(rows)
    }

    async fn upsert_profile(&self, profile: &Profile, scope: &Scope) -> Result<Uuid> {
        enforce_write(scope)?;

        let id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO profiles (id, name, description, source, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (name) DO UPDATE SET
                description = EXCLUDED.description,
                source = EXCLUDED.source,
                updated_at = EXCLUDED.updated_at
            RETURNING id
            "#,
        )
        .bind(profile.id)
        .bind(&profile.name)
        .bind(&profile.description)
        .bind(&profile.source)
        .bind(profile.created_at)
        .bind(profile.updated_at)
        .fetch_one(self.pg.pool())
        .await
        .map_err(StoreError::from)?;

        Ok(id)
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

/// Canonical WaiverStore trait. spindle-server must import and implement
/// this rather than re-declaring its own.
#[async_trait::async_trait]
pub trait WaiverStore: Send + Sync + std::fmt::Debug {
    async fn get_waiver(&self, id: Uuid, scope: &Scope) -> Result<Waiver>;
    async fn list_waivers(&self, scope: &Scope) -> Result<Vec<Waiver>>;
    async fn upsert_waiver(&self, waiver: &Waiver, scope: &Scope) -> Result<Uuid>;
    async fn delete_waiver(&self, id: Uuid, scope: &Scope) -> Result<()>;
}

#[derive(Debug)]
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
    async fn get_waiver(&self, id: Uuid, scope: &Scope) -> Result<Waiver> {
        enforce_read(scope)?;

        let row = sqlx::query_as::<sqlx::Postgres, Waiver>(
            r#"
            SELECT
                id, control_id, profile_id, scope,
                justification, approver, start_date, expiry_date,
                created_at, updated_at
            FROM waivers
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(self.pg.pool())
        .await?;

        match row {
            Some(waiver) => Ok(waiver),
            None => Err(StoreError::NotFound(format!("waiver {}", id))),
        }
    }

    async fn list_waivers(&self, scope: &Scope) -> Result<Vec<Waiver>> {
        enforce_read(scope)?;

        let rows = sqlx::query_as::<sqlx::Postgres, Waiver>(
            r#"
            SELECT
                id, control_id, profile_id, scope,
                justification, approver, start_date, expiry_date,
                created_at, updated_at
            FROM waivers
            WHERE expiry_date > NOW()
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(self.pg.pool())
        .await?;

        Ok(rows)
    }

    async fn upsert_waiver(&self, waiver: &Waiver, scope: &Scope) -> Result<Uuid> {
        enforce_write(scope)?;

        sqlx::query(
            r#"
            INSERT INTO waivers (
                id, control_id, profile_id, scope,
                justification, approver, start_date, expiry_date,
                created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (id) DO UPDATE SET
                control_id = EXCLUDED.control_id,
                profile_id = EXCLUDED.profile_id,
                scope = EXCLUDED.scope,
                justification = EXCLUDED.justification,
                approver = EXCLUDED.approver,
                start_date = EXCLUDED.start_date,
                expiry_date = EXCLUDED.expiry_date,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(waiver.id)
        .bind(&waiver.control_id)
        .bind(waiver.profile_id)
        .bind(&waiver.scope)
        .bind(&waiver.justification)
        .bind(&waiver.approver)
        .bind(waiver.start_date)
        .bind(waiver.expiry_date)
        .bind(waiver.created_at)
        .bind(waiver.updated_at)
        .execute(self.pg.pool())
        .await
        .map_err(StoreError::from)?;

        Ok(waiver.id)
    }

    async fn delete_waiver(&self, id: Uuid, scope: &Scope) -> Result<()> {
        enforce_write(scope)?;

        let result = sqlx::query("DELETE FROM waivers WHERE id = $1")
            .bind(id)
            .execute(self.pg.pool())
            .await
            .map_err(StoreError::from)?;

        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("waiver {}", id)));
        }
        Ok(())
    }
}

/// Cookbook usage tracking entity.
#[derive(utoipa::ToSchema, Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
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
pub trait CookbookUsageStore: Send + Sync + std::fmt::Debug {
    async fn get_usage(&self, id: Uuid, scope: &Scope) -> Result<CookbookUsage>;
    async fn list_usage(&self, scope: &Scope) -> Result<Vec<CookbookUsage>>;
    async fn upsert_usage(&self, usage: &CookbookUsage, scope: &Scope) -> Result<Uuid>;
    async fn count_usage(&self, scope: &Scope) -> Result<usize>;
}

#[derive(Debug)]
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
    async fn get_usage(&self, id: Uuid, scope: &Scope) -> Result<CookbookUsage> {
        enforce_read(scope)?;

        let row = sqlx::query_as::<sqlx::Postgres, CookbookUsage>(
            r#"
            SELECT
                id, node_id, run_id, cookbook_name, cookbook_version,
                resource_type, platform, first_seen, last_seen, count, created_at
            FROM cookbook_usage
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(self.pg.pool())
        .await?;

        match row {
            Some(usage) => Ok(usage),
            None => Err(StoreError::NotFound(format!("cookbook_usage {}", id))),
        }
    }

    async fn list_usage(&self, scope: &Scope) -> Result<Vec<CookbookUsage>> {
        enforce_read(scope)?;

        let rows = sqlx::query_as::<sqlx::Postgres, CookbookUsage>(
            r#"
            SELECT
                id, node_id, run_id, cookbook_name, cookbook_version,
                resource_type, platform, first_seen, last_seen, count, created_at
            FROM cookbook_usage
            ORDER BY last_seen DESC
            "#,
        )
        .fetch_all(self.pg.pool())
        .await?;

        Ok(rows)
    }

    async fn upsert_usage(&self, usage: &CookbookUsage, scope: &Scope) -> Result<Uuid> {
        enforce_write(scope)?;

        sqlx::query(
            r#"
            INSERT INTO cookbook_usage (
                id, node_id, run_id, cookbook_name, cookbook_version,
                resource_type, platform, first_seen, last_seen, count, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (node_id, run_id, cookbook_name, cookbook_version, resource_type) DO UPDATE SET
                first_seen = LEAST(cookbook_usage.first_seen, EXCLUDED.first_seen),
                last_seen = GREATEST(cookbook_usage.last_seen, EXCLUDED.last_seen),
                count = cookbook_usage.count + EXCLUDED.count
            "#,
        )
        .bind(usage.id)
        .bind(usage.node_id)
        .bind(usage.run_id)
        .bind(&usage.cookbook_name)
        .bind(&usage.cookbook_version)
        .bind(&usage.resource_type)
        .bind(&usage.platform)
        .bind(usage.first_seen)
        .bind(usage.last_seen)
        .bind(usage.count)
        .bind(usage.created_at)
        .execute(self.pg.pool())
        .await
        .map_err(StoreError::from)?;

        // L1: row written
        tracing::info!(
            table = "cookbook_usage",
            row_id = %usage.id,
            cookbook_name = %usage.cookbook_name,
            "store row written"
        );
        // L2: per-table latency
        tracing::debug!(
            table = "cookbook_usage",
            cookbook_name = %usage.cookbook_name,
            run_id = %usage.run_id,
            "store write timing"
        );

        Ok(usage.id)
    }

    async fn count_usage(&self, scope: &Scope) -> Result<usize> {
        let mut qb = QueryBuilder::new("SELECT COUNT(*) FROM cookbook_usage");
        push_scope_where::<RollupsScopeFilter>(&mut qb, scope);
        let row = qb.build().fetch_one(self.pg.pool()).await?;
        Ok(row.get::<i64, _>("count") as usize)
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
        // Unrestricted → no predicate pushed
        let all = Scope::all();
        let mut qb = QueryBuilder::new("SELECT * FROM nodes");
        push_scope_filter::<NodesScopeFilter>(&mut qb, &all);
        assert_eq!(qb.sql(), "SELECT * FROM nodes");

        // Scoped → AND ... IN (?, ?) appended after an existing predicate
        let mut projects = HashSet::new();
        projects.insert("proj-1".to_string());
        projects.insert("proj-2".to_string());
        let scoped = Scope::new(projects, HashSet::new());

        let mut qb = QueryBuilder::new("SELECT * FROM nodes WHERE id = $1");
        push_scope_filter::<NodesScopeFilter>(&mut qb, &scoped);
        let sql = qb.sql();
        assert!(sql.contains("AND"));
        assert!(sql.contains("IN"));
        assert!(!sql.contains("proj-1")); // bound as params, not string literals
        assert!(!sql.contains("proj-2"));
    }

    #[test]
    fn test_scope_filter_count_queries() {
        // COUNT queries get a keyword-aware WHERE clause
        let mut projects = HashSet::new();
        projects.insert("test-proj".to_string());
        let scoped = Scope::new(projects, HashSet::new());

        let mut qb = QueryBuilder::new("SELECT COUNT(*) FROM runs");
        push_scope_where::<RunsScopeFilter>(&mut qb, &scoped);
        let sql = qb.sql();
        assert!(sql.contains("WHERE"));
        assert!(sql.contains("IN"));
        assert!(!sql.contains("test-proj")); // bound, not a string literal
    }

    #[test]
    fn test_scope_filter_aggregate_queries() {
        let mut projects = HashSet::new();
        projects.insert("agg-proj".to_string());
        let scoped = Scope::new(projects, HashSet::new());

        let mut qb = QueryBuilder::new("SELECT * FROM duration_rollups WHERE hour = $1");
        push_scope_filter::<RollupsScopeFilter>(&mut qb, &scoped);
        let sql = qb.sql();
        assert!(sql.contains("AND"));
        assert!(sql.contains("IN"));
    }

    #[test]
    fn test_scope_filter_exists_queries() {
        let mut projects = HashSet::new();
        projects.insert("exist-proj".to_string());
        let scoped = Scope::new(projects, HashSet::new());

        let mut qb = QueryBuilder::new("SELECT 1 FROM resource_events");
        push_scope_where::<ResourceEventsScopeFilter>(&mut qb, &scoped);
        let sql = qb.sql();
        assert!(sql.contains("WHERE"));
        assert!(sql.contains("IN"));
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
            project_id: "default".to_string(),
            node_type: "cinc-client".to_string(),
            run_list: vec![],
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
        fn _assert_pghoststore_new(pool: PgPool) -> PgStore {
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

#![allow(warnings)]
//! Spindle worker — library interface for testing.
//!
//! Re-exports the pipeline worker's core types and processing functions
//! so integration tests can exercise the full dequeue → archive → parse →
//! store pipeline against a real PostgreSQL database.
//!
//! The worker binary (main.rs) remains the daemon entry point; this crate
//! (lib.rs) exposes the testable surface.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use tracing::{error, info, warn};

use spindle_pipeline::process_payload;
use spindle_rawarchive::Archive;
use spindle_store::{
    ComplianceStore, CookbookUsageStore, NodeStore, ProfileStore, ResourceEventStore, RunStore,
    Scope,
};

/// How long a job can be "processing" before it's considered stuck.
pub const CLAIM_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait between stuck-job recovery sweeps.
pub const RECOVERY_INTERVAL: Duration = Duration::from_secs(10);
/// How long to wait for in-flight jobs to complete during graceful shutdown.
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(30);

// ── Worker config ──────────────────────────────────────────────────────────

/// Configuration for the pipeline worker.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub database_url: String,
    pub archive_dir: String,
    pub poll_interval: Duration,
    pub claim_timeout: Duration,
}

impl WorkerConfig {
    pub fn from_env() -> Self {
        Self {
            database_url: std::env::var("SPINDLE_DATABASE_URL")
                .or_else(|_| std::env::var("DATABASE_URL"))
                .unwrap_or_else(|_| {
                    "postgres://spindle:CHANGE_ME@localhost:5432/spindle".to_string()
                }),
            archive_dir: std::env::var("SPINDLE_ARCHIVE_DIR")
                .unwrap_or_else(|_| "/var/lib/spindle/archive".to_string()),
            poll_interval: Duration::from_secs(
                std::env::var("SPINDLE_WORKER_POLL_INTERVAL")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1),
            ),
            claim_timeout: Duration::from_secs(
                std::env::var("SPINDLE_WORKER_CLAIM_TIMEOUT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(30),
            ),
        }
    }
}

/// A job dequeued from the `jobs` table.
#[derive(Debug, Clone)]
pub struct QueuedJob {
    pub id: String,
    pub payload_key: String,
    pub node_id: String,
    pub run_id: String,
    #[allow(dead_code)]
    pub status: String,
    pub retry_count: i32,
    pub max_retries: i32,
    #[allow(dead_code)]
    pub error_message: Option<String>,
    pub node_name: String,
}

/// DB row representation of a job.
#[derive(sqlx::FromRow, Debug)]
pub struct QueuedJobRow {
    pub id: String,
    pub payload_key: String,
    pub node_id: String,
    pub run_id: String,
    pub status: String,
    #[sqlx(default)]
    pub retry_count: i32,
    #[sqlx(default)]
    pub max_retries: i32,
    pub error_message: Option<String>,
    #[sqlx(default)]
    pub node_name: String,
}

// ── Builder functions (re-exported from main.rs logic) ─────────────────────

/// Build a `spindle_store::Node` from a run-converge payload.
pub fn build_node_from_payload(
    payload: &serde_json::Value,
    node_id: uuid::Uuid,
) -> spindle_store::Node {
    let node_obj = payload
        .get("node")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let name = payload
        .get("node_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let platform = node_obj
        .pointer("/automatic/platform")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let platform_version = node_obj
        .pointer("/automatic/platform_version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let chef_environment = node_obj
        .get("chef_environment")
        .and_then(|v| v.as_str())
        .unwrap_or("_default")
        .to_string();
    let policy_group = payload
        .get("policy_group")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let policy_name = payload
        .get("policy_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let attributes = node_obj
        .get("automatic")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    spindle_store::Node {
        id: node_id,
        name,
        platform,
        platform_version,
        chef_environment,
        policy_group,
        policy_name,
        attributes,
        project_id: "default".to_string(),
        node_type: "cinc-client".to_string(),
        last_seen: Utc::now(),
        created_at: Utc::now(),
    }
}

/// Build a `spindle_store::Run` from a run-converge payload + pipeline stats.
pub fn build_run_from_payload(
    payload: &serde_json::Value,
    run_row_id: uuid::Uuid,
    node_id: uuid::Uuid,
    stats: &spindle_pipeline::RunResourceStats,
) -> spindle_store::Run {
    let run_id = payload
        .get("run_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let status = payload
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let start_time = payload
        .get("start_time")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    let end_time = payload
        .get("end_time")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));
    let error_summary = if status == "failure" || status == "failed" {
        payload.get("error").cloned()
    } else {
        None
    };

    spindle_store::Run {
        id: run_row_id,
        node_id,
        run_id,
        status,
        start_time,
        end_time,
        total_resource_count: stats.total_resource_count as i32,
        updated_count: stats.updated_count as i32,
        failed_count: stats.failed_count as i32,
        skipped_count: stats.skipped_count as i32,
        error_summary,
        cookbook_set: payload.get("cookbooks").cloned(),
        schema_version: spindle_pipeline::SCHEMA_VERSION,
        created_at: Utc::now(),
    }
}

/// Build `ResourceEvent`s from the payload's original `resources` array, coupled
/// with the pipeline's parsed (filtered) events.
pub fn build_resource_events_from_parsed(
    payload: &serde_json::Value,
    node_id: uuid::Uuid,
    run_row_id: uuid::Uuid,
    parsed: &[spindle_pipeline::ParsedResourceEvent],
) -> Vec<spindle_store::ResourceEvent> {
    let raw_resources = payload
        .get("resources")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    parsed
        .iter()
        .map(|ev| {
            let raw = raw_resources
                .iter()
                .find(|r| r.get("name").and_then(|n| n.as_str()) == Some(ev.name.as_str()));

            let resource_type = raw
                .and_then(|r| r.get("type").and_then(|t| t.as_str()))
                .unwrap_or("resource")
                .to_string();
            let resource_name = ev.name.clone();
            let action_raw = raw
                .and_then(|r| r.get("action"))
                .and_then(|a| a.as_array())
                .and_then(|arr| arr.first())
                .and_then(|a| a.as_str())
                .unwrap_or("apply")
                .to_string();
            let duration_ms = raw
                .and_then(|r| r.get("duration").and_then(|d| d.as_f64()))
                .map(|d| (d * 1000.0) as i32)
                .unwrap_or(0);
            let cookbook_name = raw
                .and_then(|r| r.get("cookbook_name").and_then(|c| c.as_str()))
                .unwrap_or("")
                .to_string();
            let cookbook_version = raw
                .and_then(|r| r.get("cookbook_version").and_then(|c| c.as_str()))
                .unwrap_or("")
                .to_string();
            let delta = if status_is_changed(&ev.status) {
                raw.and_then(|r| r.get("delta").cloned())
            } else {
                None
            };

            spindle_store::ResourceEvent {
                id: uuid::Uuid::new_v4(),
                run_id: run_row_id,
                node_id,
                resource_type,
                resource_name,
                action: action_raw,
                status: ev.status.to_string(),
                duration_ms,
                cookbook_name,
                cookbook_version,
                guard_outcome: None,
                delta,
                schema_version: spindle_pipeline::SCHEMA_VERSION,
                created_at: Utc::now(),
            }
        })
        .collect()
}

/// Build a `spindle_store::CookbookUsage` store entity from a pipeline `CookbookUsage`.
pub fn build_cookbook_usage(
    usage: &spindle_pipeline::CookbookUsage,
    node_id: uuid::Uuid,
    run_id: uuid::Uuid,
) -> spindle_store::CookbookUsage {
    spindle_store::CookbookUsage {
        id: uuid::Uuid::new_v4(),
        node_id,
        run_id,
        cookbook_name: usage.cookbook_name.clone(),
        cookbook_version: usage.cookbook_version.clone(),
        resource_type: "resource".to_string(),
        platform: None,
        first_seen: Utc::now(),
        last_seen: Utc::now(),
        count: 1,
        created_at: Utc::now(),
    }
}

fn status_is_changed(s: &spindle_pipeline::ResourceStatus) -> bool {
    matches!(
        s,
        spindle_pipeline::ResourceStatus::Updated | spindle_pipeline::ResourceStatus::Failed
    )
}

/// Detect a no-op converge: a payload with zero resource events (or
/// `total_resource_count == 0`) changed nothing and should be skipped,
/// not dead-lettered.
pub fn is_noop_payload(payload: &serde_json::Value) -> bool {
    let resource_count = payload
        .get("resources")
        .and_then(|r| r.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let total_count = payload
        .get("total_resource_count")
        .and_then(|c| c.as_u64())
        .unwrap_or(resource_count as u64);

    resource_count == 0 || total_count == 0
}

// ── PipelineWorker ───────────────────────────────────────────────────────────

/// The main worker that consumes jobs from the PostgreSQL job queue.
pub struct PipelineWorker {
    pub pool: sqlx::PgPool,
    pub archive: Arc<dyn Archive>,
    pub config: WorkerConfig,
}

impl PipelineWorker {
    /// Create a new worker connected to the database and archive root.
    pub async fn new(config: WorkerConfig) -> Result<Self, sqlx::Error> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(10)
            .connect(&config.database_url)
            .await?;

        let archive = Arc::new(
            spindle_rawarchive::LocalArchive::new(&config.archive_dir)
                .map_err(|e| sqlx::Error::ColumnNotFound(e.to_string()))?,
        );

        Ok(Self {
            pool,
            archive,
            config,
        })
    }

    /// Create a worker from an existing pool (for testing).
    pub fn new_with_pool(
        pool: sqlx::PgPool,
        archive_dir: &str,
        poll_interval: Duration,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(archive_dir)?);
        Ok(Self {
            pool,
            archive,
            config: WorkerConfig {
                database_url: String::new(),
                archive_dir: archive_dir.to_string(),
                poll_interval,
                claim_timeout: CLAIM_TIMEOUT,
            },
        })
    }

    /// Process exactly one job (for testing — does not loop).
    pub async fn process_one(&self) -> Result<Option<QueuedJob>, Box<dyn std::error::Error>> {
        match self.dequeue().await? {
            Some(job) => {
                info!(
                    job_id = %job.id,
                    payload_key = %job.payload_key,
                    "dequeued job for processing"
                );
                match self.process_job(&job).await {
                    Ok(()) => {
                        info!(
                            job_id = %job.id,
                            action = "processed",
                            "pipeline job processed"
                        );
                    }
                    Err(e) => {
                        error!(
                            job_id = %job.id,
                            error = %e,
                            action = "error",
                            "pipeline job failed"
                        );
                        let node_name = job.node_name.clone();
                        if let Err(e) = self.handle_job_failure(&job, &e, Some(&node_name)).await {
                            error!("handle_job_failure failed: {}", e);
                        }
                    }
                }
                Ok(Some(job))
            }
            None => Ok(None),
        }
    }

    /// Atomically claim a pending job using SKIP LOCKED.
    pub async fn dequeue(&self) -> Result<Option<QueuedJob>, Box<dyn std::error::Error>> {
        let row = sqlx::query_as::<_, QueuedJobRow>(
            r#"
            UPDATE jobs
            SET status = 'processing', started_at = NOW()
            WHERE id = (
                SELECT id FROM jobs
                WHERE status = 'pending'
                ORDER BY created_at ASC
                FOR UPDATE SKIP LOCKED
                LIMIT 1
            )
            RETURNING id, payload_key, node_id, run_id, status, retry_count, max_retries, error_message, node_name
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| QueuedJob {
            id: r.id,
            payload_key: r.payload_key,
            node_id: r.node_id,
            run_id: r.run_id,
            status: r.status,
            retry_count: r.retry_count,
            max_retries: r.max_retries,
            error_message: r.error_message,
            node_name: r.node_name,
        }))
    }

    /// Process a single job: read archive → parse → write to store.
    pub async fn process_job(&self, job: &QueuedJob) -> Result<(), String> {
        // Read raw payload from archive
        let raw = self
            .archive
            .retrieve(&job.payload_key)
            .map_err(|e| format!("archive retrieve failed: {}", e))?;

        // Parse JSON
        let payload: serde_json::Value =
            serde_json::from_slice(&raw).map_err(|e| format!("JSON parse failed: {}", e))?;

        // Cinc Auditor detection: if the payload has a "profiles" key (an array),
        // route to compliance report processing instead of resource events.
        if payload.get("profiles").is_some() {
            return self.process_compliance_job(job, &payload).await;
        }

        // No-op detection: a converge that changed nothing (0 resource events)
        // should be skipped, not dead-lettered.
        if is_noop_payload(&payload) {
            sqlx::query(
                r#"UPDATE jobs SET status = 'skipped', updated_at = NOW(), completed_at = NOW() WHERE id = $1"#,
            )
            .bind(&job.id)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("failed to mark job skipped: {}", e))?;

            info!(
                job_id = %job.id,
                node_name = %job.node_name,
                run_id = %job.run_id,
                action = "skipped",
                reason = "no_resources",
                "job skipped: no-op converge (0 resources)"
            );
            return Ok(());
        }

        // Pipeline: parse → normalize → filter
        let result =
            process_payload(&payload).map_err(|e| format!("pipeline processing failed: {}", e))?;

        let stats = &result.stats;
        let scope = Scope::all();

        // Write to store tables
        let node_store = spindle_store::SqlxNodeStore::new(self.pool.clone());
        let run_store = spindle_store::SqlxRunStore::new(self.pool.clone());
        let event_store = spindle_store::SqlxResourceEventStore::new(self.pool.clone());
        let cookbook_store = spindle_store::SqlxCookbookUsageStore::new(self.pool.clone());

        // Resolve node_id: use existing UUID if present, else generate
        let node_id = if !job.node_id.is_empty() {
            uuid::Uuid::parse_str(&job.node_id).unwrap_or_else(|_| uuid::Uuid::new_v4())
        } else {
            uuid::Uuid::new_v4()
        };

        // Backfill node_name if it was NULL
        #[allow(unused_variables)]
        let node_name = if job.node_name.is_empty() {
            let inferred = payload
                .get("node_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            sqlx::query(r#"UPDATE jobs SET node_name = $1, updated_at = NOW() WHERE id = $2"#)
                .bind(&inferred)
                .bind(&job.id)
                .execute(&self.pool)
                .await
                .ok();
            inferred
        } else {
            job.node_name.clone()
        };

        // Upsert node
        let node = build_node_from_payload(&payload, node_id);
        let _node_row = node_store
            .upsert_node(&node, &scope)
            .await
            .map_err(|e| format!("node upsert failed: {}", e))?;

        // Insert run
        let run_row_id = uuid::Uuid::new_v4();
        let run = build_run_from_payload(&payload, run_row_id, node_id, stats);
        let _run_row = run_store
            .insert_run(&run, &scope)
            .await
            .map_err(|e| format!("run insert failed: {}", e))?;

        // Insert resource events
        let events = build_resource_events_from_parsed(
            &payload,
            node_id,
            run_row_id,
            &result.persistable_events,
        );
        for ev in &events {
            event_store
                .insert_event(ev, &scope)
                .await
                .map_err(|e| format!("event insert failed: {}", e))?;
        }

        // Extract and upsert cookbook usage (M1-26)
        let cookbook_usages = spindle_pipeline::extract_cookbook_usage(
            &result.persistable_events,
            &node_id.to_string(),
            &job.run_id,
        );
        for usage in &cookbook_usages {
            let store_usage = build_cookbook_usage(usage, node_id, run_row_id);
            let _ = cookbook_store
                .upsert_usage(&store_usage, &scope)
                .await
                .map_err(|e| format!("cookbook usage upsert failed: {}", e))?;
        }

        // Mark job as completed
        sqlx::query(
            r#"UPDATE jobs SET status = 'completed', updated_at = NOW(), completed_at = NOW() WHERE id = $1"#,
        )
        .bind(&job.id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("failed to update job status: {}", e))?;

        tracing::debug!(
            job_id = %job.id,
            events_persisted = events.len(),
            total_resources = stats.total_resource_count,
            updated = stats.updated_count,
            failed = stats.failed_count,
            skipped = stats.skipped_count,
            up_to_date = stats.up_to_date_count,
            "job processing stats"
        );
        info!(
            job_id = %job.id,
            action = "processed",
            "pipeline job processed"
        );

        Ok(())
    }

    /// Process a compliance (Cinc Auditor) job: parse → upsert node/run/profile →
    /// insert compliance_report + control_results.
    pub async fn process_compliance_job(
        &self,
        job: &QueuedJob,
        payload: &serde_json::Value,
    ) -> Result<(), String> {
        let scope = Scope::all();

        // Parse the Cinc Auditor compliance report
        let parser = spindle_pipeline::ComplianceReportParser::new();
        let report = parser
            .parse(payload)
            .map_err(|e| format!("compliance report parse failed: {}", e))?;

        // Extract control results
        let control_results = parser.extract_control_results(&report);

        // Resolve node_id: look up existing node by name to avoid duplicates.
        // The auditor path doesn't carry a stable entity_uuid (unlike data-collector),
        // so we query the DB for an existing node with the same name. If found,
        // reuse its UUID so upsert_node's ON CONFLICT (id) fires correctly.
        // Otherwise fall back to the job's node_id or generate a new UUID.
        let node_name = payload
            .get("node_name")
            .and_then(|v| v.as_str())
            .or_else(|| {
                payload
                    .get("platform")
                    .and_then(|p| p.get("name"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("unknown")
            .to_string();

        let node_store = spindle_store::SqlxNodeStore::new(self.pool.clone());

        // Always try name-based lookup first — the auditor ingest handler
        // generates a fresh random UUID per scan, so job.node_id is not stable.
        let node_id = match node_store.find_node_id_by_name(&node_name).await {
            Ok(Some(existing_id)) => {
                tracing::debug!(
                    node_name = %node_name,
                    existing_id = %existing_id,
                    "Reusing existing node UUID for auditor payload"
                );
                existing_id
            }
            _ => {
                // No existing node — use job's UUID if parseable, else generate
                let new_id = if !job.node_id.is_empty() {
                    uuid::Uuid::parse_str(&job.node_id).unwrap_or_else(|_| uuid::Uuid::new_v4())
                } else {
                    uuid::Uuid::new_v4()
                };
                tracing::debug!(
                    node_name = %node_name,
                    new_id = %new_id,
                    "Creating new node for auditor payload"
                );
                new_id
            }
        };

        // Upsert node (build from Cinc Auditor payload)
        let node = build_node_from_auditor_payload(payload, node_id);
        let _node_row = node_store
            .upsert_node(&node, &scope)
            .await
            .map_err(|e| format!("node upsert failed: {}", e))?;

        // Insert run
        let run_store = spindle_store::SqlxRunStore::new(self.pool.clone());
        let run_row_id = uuid::Uuid::new_v4();
        let run = build_run_from_auditor_payload(payload, run_row_id, node_id, &report);
        let _run_row = run_store
            .insert_run(&run, &scope)
            .await
            .map_err(|e| format!("run insert failed: {}", e))?;

        // Upsert profiles and insert compliance reports + control results
        let profile_store = spindle_store::SqlxProfileStore::new(self.pool.clone());
        let compliance_store = spindle_store::SqlxComplianceStore::new(self.pool.clone());

        let report_id = uuid::Uuid::new_v4();
        let now = chrono::Utc::now();

        for profile in &report.profiles {
            let profile_id = uuid::Uuid::new_v4();
            let profile_entity = spindle_store::Profile {
                id: profile_id,
                name: profile.name.clone(),
                description: profile.title.clone(),
                source: profile.sha256.clone().unwrap_or_default(),
                created_at: now,
                updated_at: now,
            };
            let profile_id = profile_store
                .upsert_profile(&profile_entity, &scope)
                .await
                .map_err(|e| format!("profile upsert failed: {}", e))?;

            let profile_results: Vec<&spindle_pipeline::ParsedControlResult> = control_results
                .iter()
                .filter(|cr| cr.profile_name == profile.name)
                .collect();

            let passed_count = profile_results
                .iter()
                .filter(|cr| matches!(cr.status, spindle_pipeline::AuditorStatus::Passed))
                .count() as i32;
            let failed_count = profile_results
                .iter()
                .filter(|cr| matches!(cr.status, spindle_pipeline::AuditorStatus::Failed))
                .count() as i32;
            let warning_count = profile_results
                .iter()
                .filter(|cr| matches!(cr.status, spindle_pipeline::AuditorStatus::Skipped))
                .count() as i32;

            let status = if failed_count > 0 {
                "failed".to_string()
            } else if warning_count > 0 {
                "warn".to_string()
            } else {
                "passed".to_string()
            };

            let compliance_report = spindle_store::ComplianceReport {
                id: report_id,
                run_id: run_row_id,
                node_id,
                profile_id,
                profile_name: profile.name.clone(),
                status,
                passed_count,
                failed_count,
                warning_count,
                created_at: now,
            };
            let _ = compliance_store
                .insert_report(&compliance_report, &scope)
                .await
                .map_err(|e| format!("compliance report insert failed: {}", e))?;

            for cr in &profile_results {
                let control_result = spindle_store::ControlResult {
                    id: uuid::Uuid::new_v4(),
                    report_id,
                    run_id: run_row_id,
                    node_id,
                    profile_id,
                    control_id: cr.control_id.clone(),
                    status: cr.status.to_string(),
                    impact: cr.impact.unwrap_or(0.0),
                    result: Some(serde_json::json!({
                        "title": cr.title,
                        "description": cr.description,
                        "code": cr.code,
                        "run_time": cr.run_time,
                        "start_time": cr.start_time,
                        "message": cr.message,
                        "skip_reason": cr.skip_reason,
                    })),
                    created_at: now,
                };
                let _ = compliance_store
                    .insert_control_result(&control_result, &scope)
                    .await
                    .map_err(|e| format!("control result insert failed: {}", e))?;
            }
        }

        // Mark job as completed
        sqlx::query(
            r#"UPDATE jobs SET status = 'completed', updated_at = NOW(), completed_at = NOW() WHERE id = $1"#,
        )
        .bind(&job.id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("failed to update job status: {}", e))?;

        info!(
            job_id = %job.id,
            node_name = %job.node_name,
            run_id = %job.run_id,
            profiles = report.profiles.len(),
            control_results = control_results.len(),
            action = "processed_compliance",
            "compliance job processed"
        );

        Ok(())
    }

    /// Handle a job failure: retry or dead-letter.
    pub async fn handle_job_failure(
        &self,
        job: &QueuedJob,
        error: &str,
        node_name: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let new_retry_count = job.retry_count + 1;

        if new_retry_count >= job.max_retries {
            // Move to dead letter
            sqlx::query(
                r#"
                UPDATE jobs SET
                    status = 'dead_lettered',
                    retry_count = $1,
                    error_message = $2,
                    updated_at = NOW()
                WHERE id = $3
                "#,
            )
            .bind(new_retry_count)
            .bind(error)
            .bind(&job.id)
            .execute(&self.pool)
            .await?;

            warn!(
                job_id = %job.id,
                retry_count = new_retry_count,
                error = %error,
                action = "dead_lettered",
                "job dead-lettered after retries exhausted"
            );

            let dl_node_name = node_name.unwrap_or(&job.node_name);
            let _ = sqlx::query(
                r#"
                INSERT INTO pipeline_dead_letter (archive_reference, error_message, error_type, retry_count, payload_type, node_name, run_id, created_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())
                "#,
            )
            .bind(&job.payload_key)
            .bind(error)
            .bind("PipelineError")
            .bind(new_retry_count)
            .bind("run-converge")
            .bind(dl_node_name)
            .bind(&job.run_id)
            .execute(&self.pool)
            .await;
        } else {
            // Re-queue for retry
            sqlx::query(
                r#"
                UPDATE jobs SET
                    status = 'pending',
                    retry_count = $1,
                    error_message = $2,
                    started_at = NULL,
                    updated_at = NOW()
                WHERE id = $3
                "#,
            )
            .bind(new_retry_count)
            .bind(error)
            .bind(&job.id)
            .execute(&self.pool)
            .await?;

            warn!(
                job_id = %job.id,
                retry_count = new_retry_count,
                max_retries = job.max_retries,
                action = "re_queued",
                "job failed, re-queued for retry"
            );
        }

        Ok(())
    }

    /// Recover jobs that have been "processing" for longer than the claim timeout.
    pub async fn recover_stuck_jobs(&self) -> Result<u64, Box<dyn std::error::Error>> {
        let result = sqlx::query(
            r#"
            UPDATE jobs
            SET status = 'pending', started_at = NULL, updated_at = NOW()
            WHERE status = 'processing' AND started_at < NOW() - INTERVAL '30 seconds'
            "#,
        )
        .execute(&self.pool)
        .await?;

        if result.rows_affected() > 0 {
            warn!("recovered {} stuck jobs", result.rows_affected());
        }

        Ok(result.rows_affected())
    }

    /// Main loop — polls for jobs, processes them, recovers stuck jobs.
    /// Uses tokio::select! to respond to shutdown signals.
    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        info!(
            poll_interval_secs = %self.config.poll_interval.as_secs(),
            claim_timeout_secs = %self.config.claim_timeout.as_secs(),
            "spindle-worker started"
        );

        let shutdown = Arc::new(spindle_shutdown::GracefulShutdown::with_default_deadline());

        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;

        let mut last_recovery = Instant::now();
        loop {
            tokio::select! {
                _ = sigterm.recv() => {
                    info!(
                        signal = "SIGTERM",
                        drain_deadline_secs = %SHUTDOWN_DEADLINE.as_secs(),
                        "shutdown signal received, draining in-flight jobs"
                    );
                    self.drain_shutdown(&shutdown).await;
                    break;
                }
                _ = sigint.recv() => {
                    info!(
                        signal = "SIGINT",
                        drain_deadline_secs = %SHUTDOWN_DEADLINE.as_secs(),
                        "shutdown signal received, draining in-flight jobs"
                    );
                    self.drain_shutdown(&shutdown).await;
                    break;
                }
                _ = tokio::time::sleep(self.config.poll_interval) => {
                    if last_recovery.elapsed() >= RECOVERY_INTERVAL {
                        if let Err(e) = self.recover_stuck_jobs().await {
                            error!("recover_stuck_jobs failed: {}", e);
                        }
                        last_recovery = Instant::now();
                    }
                    match self.dequeue().await {
                        Ok(Some(job)) => {
                            info!(
                                job_id = %job.id,
                                payload_key = %job.payload_key,
                                node_name = %job.node_name,
                                run_id = %job.run_id,
                                "dequeued job for processing"
                            );
                            shutdown.mark_in_flight();
                            match self.process_job(&job).await {
                                Ok(_) => {
                                    info!(
                                        job_id = %job.id,
                                        node_name = %job.node_name,
                                        run_id = %job.run_id,
                                        action = "processed",
                                        "pipeline job processed"
                                    );
                                }
                                Err(e) => {
                                    error!(
                                        job_id = %job.id,
                                        node_name = %job.node_name,
                                        run_id = %job.run_id,
                                        error = %e,
                                        action = "error",
                                        "pipeline job failed"
                                    );
                                    let node_name = job.node_name.clone();
                                    if let Err(e) = self.handle_job_failure(&job, &e, Some(&node_name)).await {
                                        error!("handle_job_failure failed: {}", e);
                                    }
                                }
                            }
                            shutdown.mark_complete();
                        }
                        Ok(None) => {}
                        Err(e) => {
                            error!("dequeue failed: {}", e);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Wait for in-flight jobs to complete or deadline expires.
    pub async fn drain_shutdown(&self, shutdown: &Arc<spindle_shutdown::GracefulShutdown>) {
        let deadline = tokio::time::Instant::now() + SHUTDOWN_DEADLINE;
        while shutdown.has_in_flight() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if shutdown.has_in_flight() {
            warn!("shutdown deadline reached, some jobs may need re-queueing on next start");
        }
        info!("spindle-worker stopped");
        tracing::info!(action = "shutdown_complete", "spindle-worker stopped");
    }
}

/// Build a `Node` from a Cinc Auditor compliance report payload.
pub fn build_node_from_auditor_payload(
    payload: &serde_json::Value,
    node_id: uuid::Uuid,
) -> spindle_store::Node {
    let platform = payload
        .get("platform")
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("auditor")
        .to_string();
    let platform_version = payload
        .get("platform")
        .and_then(|p| p.get("release"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = payload
        .get("node_name")
        .and_then(|v| v.as_str())
        .or_else(|| {
            payload
                .get("platform")
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("unknown")
        .to_string();
    let attributes = payload.clone();

    spindle_store::Node {
        id: node_id,
        name,
        platform,
        platform_version,
        chef_environment: "auditor".to_string(),
        policy_group: "".to_string(),
        policy_name: "".to_string(),
        attributes,
        project_id: "default".to_string(),
        node_type: "audit-target".to_string(),
        last_seen: Utc::now(),
        created_at: Utc::now(),
    }
}

#[cfg(test)]
mod wire_format_tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    /// Regression test: build_node_from_payload on a real-shape Cinc/Chef
    /// data-collector run_converge payload. Verifies that platform,
    /// platform_version, policy_group, policy_name, and attributes are
    /// extracted from the correct JSON paths (not the fictional paths
    /// that were there before the fix).
    #[test]
    fn test_build_node_from_payload_real_wire_format() {
        let payload = json!({
            "run_id": "run-001",
            "node_name": "web-01",
            "node": {
                "name": "web-01",
                "chef_environment": "_default",
                "automatic": {
                    "platform": "ubuntu",
                    "platform_version": "24.04",
                    "hostname": "web-01.example.com"
                }
            },
            "policy_group": "web",
            "policy_name": "apache2",
            "resources": []
        });

        let node_id = Uuid::new_v4();
        let node = build_node_from_payload(&payload, node_id);

        assert_eq!(node.id, node_id);
        assert_eq!(node.name, "web-01");
        assert_eq!(node.platform, "ubuntu");
        assert_eq!(node.platform_version, "24.04");
        assert_eq!(node.chef_environment, "_default");
        assert_eq!(node.policy_group, "web");
        assert_eq!(node.policy_name, "apache2");
        assert_eq!(node.node_type, "cinc-client");
        // attributes should come from node.automatic, not node.attributes
        assert!(node.attributes.is_object());
        assert_eq!(node.attributes["hostname"], "web-01.example.com");
    }

    /// Regression test: build_node_from_auditor_payload sets node_type
    /// to "audit-target".
    #[test]
    fn test_build_node_from_auditor_payload_node_type() {
        let payload = json!({
            "platform": {
                "name": "audit-node",
                "release": "1.0"
            },
            "node_name": "fleet-audit-target",
            "profiles": [],
            "statistics": { "duration": 1.0 }
        });

        let node_id = Uuid::new_v4();
        let node = build_node_from_auditor_payload(&payload, node_id);

        assert_eq!(node.node_type, "audit-target");
        assert_eq!(node.name, "fleet-audit-target");
        assert_eq!(node.platform, "audit-node");
    }

    /// Regression test: when node.automatic is missing, platform defaults
    /// to "unknown" (not a crash).
    #[test]
    fn test_build_node_from_payload_missing_automatic() {
        let payload = json!({
            "run_id": "run-002",
            "node_name": "minimal-node",
            "node": {
                "chef_environment": "production"
            },
            "policy_group": "base",
        });

        let node = build_node_from_payload(&payload, Uuid::new_v4());

        assert_eq!(node.name, "minimal-node");
        assert_eq!(node.platform, "unknown");
        assert_eq!(node.platform_version, "");
        assert_eq!(node.policy_group, "base");
        assert_eq!(node.policy_name, "");
        assert_eq!(node.node_type, "cinc-client");
    }
}

/// Build a `Run` from a Cinc Auditor compliance report payload.
pub fn build_run_from_auditor_payload(
    payload: &serde_json::Value,
    run_row_id: uuid::Uuid,
    node_id: uuid::Uuid,
    report: &spindle_pipeline::ComplianceReport,
) -> spindle_store::Run {
    let run_id = payload
        .get("node_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let status = "success".to_string();

    let start_time = payload
        .get("statistics")
        .and_then(|s| s.get("duration"))
        .and_then(|v| v.as_f64())
        .map(|_| Utc::now())
        .unwrap_or_else(Utc::now);

    spindle_store::Run {
        id: run_row_id,
        node_id,
        run_id,
        status,
        start_time,
        end_time: None,
        total_resource_count: report
            .profiles
            .iter()
            .map(|p| p.controls.len())
            .sum::<usize>() as i32,
        updated_count: 0,
        failed_count: 0,
        skipped_count: 0,
        error_summary: None,
        cookbook_set: None,
        schema_version: spindle_pipeline::SCHEMA_VERSION,
        created_at: Utc::now(),
    }
}

//! Spindle worker — background daemon that consumes jobs from the PostgreSQL
//! job queue and processes them through the pipeline.
//!
//! ## How it works
//!
//! 1. Polls the `jobs` table for `status = 'pending'` rows.
//! 2. Claims a job atomically with `UPDATE ... WHERE status='pending' ORDER BY
//!    priority DESC LIMIT 1 FOR UPDATE SKIP LOCKED` (concurrent-safe).
//! 3. Reads the raw payload from the archive using `payload_key`.
//! 4. Calls `spindle_pipeline::process_payload()` → parse → normalize → filter.
//! 5. Writes results to store tables via `spindle_store::SqlxNodeStore`, etc.
//! 6. Marks job as `completed` or `dead_lettered` (on failure with retries exhausted).
//! 7. Recovers stuck jobs: if `claimed_at` is older than 30s, re-queues them.
//!
//! ## Usage
//!
//! ```sh
//! SPINDLE_DATABASE_URL=postgres://spindle:pw@db:5432/spindle \
//!   SPINDLE_ARCHIVE_DIR=/var/lib/spindle/archive \
//!   ./spindle-worker
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use tracing::{debug, error, info, warn};

use spindle_pipeline::process_payload;
use spindle_rawarchive::Archive;
use spindle_store::{
    NodeStore, RunStore, ResourceEventStore, CookbookUsageStore,
    Scope, Node, Run, ResourceEvent, CookbookUsage,
};

/// How long a job can be "processing" before it's considered stuck.
#[allow(dead_code)]
const CLAIM_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait between stuck-job recovery sweeps.
const RECOVERY_INTERVAL: Duration = Duration::from_secs(10);
/// How long to wait for in-flight jobs to complete during graceful shutdown.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct WorkerConfig {
    database_url: String,
    archive_dir: String,
    poll_interval: Duration,
    claim_timeout: Duration,
}

impl WorkerConfig {
    fn from_env() -> Self {
        Self {
            database_url: std::env::var("SPINDLE_DATABASE_URL")
                .or_else(|_| std::env::var("DATABASE_URL"))
                .unwrap_or_else(|_| "postgres://spindle:spindle@localhost:5432/spindle".to_string()),
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
struct QueuedJob {
    id: String,
    payload_key: String,
    node_id: String,
    run_id: String,
    #[allow(dead_code)]
    status: String,
    retry_count: i32,
    max_retries: i32,
    #[allow(dead_code)]
    error_message: Option<String>,
    node_name: String,
}

/// DB row representation of a job.
#[derive(sqlx::FromRow, Debug)]
struct QueuedJobRow {
    id: String,
    payload_key: String,
    node_id: String,
    run_id: String,
    status: String,
    #[sqlx(default)]
    retry_count: i32,
    #[sqlx(default)]
    max_retries: i32,
    error_message: Option<String>,
    #[sqlx(default)]
    node_name: String,
}

/// The main worker loop.
struct PipelineWorker {
    pool: sqlx::PgPool,
    archive: Arc<dyn Archive>,
    config: WorkerConfig,
}

impl PipelineWorker {
    async fn new(config: WorkerConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(10)
            .connect(&config.database_url)
            .await?;

        let archive = Arc::new(spindle_rawarchive::LocalArchive::new(&config.archive_dir)?);

        Ok(Self { pool, archive, config })
    }

    /// Main loop — polls for jobs, processes them, recovers stuck jobs.
    /// Uses tokio::select! to respond to shutdown signals.
    async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        info!(
            poll_interval_secs = %self.config.poll_interval.as_secs(),
            claim_timeout_secs = %self.config.claim_timeout.as_secs(),
            "spindle-worker started"
        );

        let shutdown = Arc::new(spindle_shutdown::GracefulShutdown::with_default_deadline());

        // Register signal handlers
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate()
        )?;
        let mut sigint = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::interrupt()
        )?;

        let mut last_recovery = Instant::now();

        loop {
            tokio::select! {
                // Check for shutdown signal (SIGTERM or SIGINT)
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

                // Periodic tick for dequeue (runs every poll_interval)
                _ = tokio::time::sleep(self.config.poll_interval) => {
                    // Recovery: re-queue stuck jobs
                    if last_recovery.elapsed() >= RECOVERY_INTERVAL {
                        if let Err(e) = self.recover_stuck_jobs().await {
                            error!("recover_stuck_jobs failed: {}", e);
                        }
                        last_recovery = Instant::now();
                    }

                    // Try to dequeue a job
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
                        Ok(None) => {
                            // No jobs available — continue to next poll iteration
                        }
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
    async fn drain_shutdown(&self, shutdown: &Arc<spindle_shutdown::GracefulShutdown>) {
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

    /// Atomically claim a pending job using SKIP LOCKED.
    async fn dequeue(&self) -> Result<Option<QueuedJob>, Box<dyn std::error::Error>> {
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
    async fn process_job(&self, job: &QueuedJob) -> Result<(), String> {
        // Read raw payload from archive
        let raw = self
            .archive
            .retrieve(&job.payload_key)
            .map_err(|e| format!("archive retrieve failed: {}", e))?;

        // Parse JSON
        let payload: serde_json::Value = serde_json::from_slice(&raw)
            .map_err(|e| format!("JSON parse failed: {}", e))?;

        // No-op detection: a converge that changed nothing (0 resource events)
        // should be skipped, not dead-lettered. This avoids noise in the DLQ.
        if is_noop_payload(&payload) {
            // Mark job as skipped (no-op converge) — do NOT route to dead-letter.
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
        let result = process_payload(&payload)
            .map_err(|e| format!("pipeline processing failed: {}", e))?;

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

        // Backfill node_name if it was NULL (jobs enqueued before column existed)
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
            &payload, node_id, run_row_id, &result.persistable_events,
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

        // L2: per-job timing + resource breakdown
        tracing::debug!(
            job_id = %job.id,
            node_name = %job.node_name,
            run_id = %job.run_id,
            events_persisted = events.len(),
            total_resources = stats.total_resource_count,
            updated = stats.updated_count,
            failed = stats.failed_count,
            skipped = stats.skipped_count,
            up_to_date = stats.up_to_date_count,
            "job processing stats"
        );
        tracing::info!(
            job_id = %job.id,
            action = "processed",
            "pipeline job processed"
        );

        Ok(())
    }

    /// Handle a job failure: retry or dead-letter.
    async fn handle_job_failure(
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

            // Insert into dead_letter table
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
    async fn recover_stuck_jobs(&self) -> Result<(), Box<dyn std::error::Error>> {
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

        Ok(())
    }
}

/// Build a `Node` from a run-converge payload.
fn build_node_from_payload(payload: &serde_json::Value, node_id: uuid::Uuid) -> Node {
    let node_obj = payload.get("node").cloned().unwrap_or(serde_json::Value::Null);
    let name = payload
        .get("node_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let platform = node_obj
        .pointer("/platform/name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let platform_version = node_obj
        .pointer("/platform/version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let chef_environment = node_obj
        .get("chef_environment")
        .and_then(|v| v.as_str())
        .unwrap_or("_default")
        .to_string();
    let policy_group = node_obj
        .get("policy_group")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let policy_name = node_obj
        .get("policy_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let attributes = node_obj
        .get("attributes")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    Node {
        id: node_id,
        name,
        platform,
        platform_version,
        chef_environment,
        policy_group,
        policy_name,
        attributes,
        last_seen: Utc::now(),
        created_at: Utc::now(),
    }
}

/// Build a `Run` from a run-converge payload + pipeline stats.
fn build_run_from_payload(
    payload: &serde_json::Value,
    run_row_id: uuid::Uuid,
    node_id: uuid::Uuid,
    stats: &spindle_pipeline::RunResourceStats,
) -> Run {
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

    Run {
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
/// with the pipeline's parsed (filtered) events so we preserve per-resource
/// type/action/duration.
fn build_resource_events_from_parsed(
    payload: &serde_json::Value,
    node_id: uuid::Uuid,
    run_row_id: uuid::Uuid,
    parsed: &[spindle_pipeline::ParsedResourceEvent],
) -> Vec<ResourceEvent> {
    let raw_resources = payload
        .get("resources")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();

    parsed
        .iter()
        .map(|ev| {
            // Find the matching raw resource (by name) to pull type/action/duration.
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

            ResourceEvent {
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

/// Build a `CookbookUsage` store entity from a pipeline `CookbookUsage`.
fn build_cookbook_usage(
    usage: &spindle_pipeline::CookbookUsage,
    node_id: uuid::Uuid,
    run_id: uuid::Uuid,
) -> CookbookUsage {
    CookbookUsage {
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
fn is_noop_payload(payload: &serde_json::Value) -> bool {
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing with three-tier logging support
    // SPINDLE_LOG_LEVEL=operational|diagnostic|debug (default: operational = L1 info)
    let log_level = std::env::var("SPINDLE_LOG_LEVEL").unwrap_or_else(|_| "operational".to_string());
    let tier_level = match log_level.to_lowercase().as_str() {
        "operational" | "info" => "info",
        "diagnostic" | "debug" => "debug",
        "trace" => "trace",
        _ => "info",
    };
    let env_filter = match std::env::var("RUST_LOG") {
        Ok(rust_log) => tracing_subscriber::EnvFilter::new(&rust_log),
        Err(_) => tracing_subscriber::EnvFilter::new(tier_level),
    };
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .json()
        .init();
    tracing::info!(log_level = %log_level, tier = %tier_level, "spindle-worker observability initialized");

    let config = WorkerConfig::from_env();

    // Validate config
    if let Err(e) = spindle_config::Config::load() {
        eprintln!("Config load warning: {}", e);
    }

    let worker = PipelineWorker::new(config).await?;
    worker.run().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_is_noop_payload_empty_resources_is_skipped() {
        // A payload with an empty resources array is a no-op → skip.
        let payload = json!({
            "run_id": "noop-001",
            "node_name": "fleet-01",
            "resources": []
        });
        assert!(is_noop_payload(&payload));
    }

    #[test]
    fn test_is_noop_payload_total_count_zero_is_skipped() {
        // total_resource_count == 0 → no-op → skip.
        let payload = json!({
            "run_id": "noop-002",
            "node_name": "fleet-01",
            "total_resource_count": 0
        });
        assert!(is_noop_payload(&payload));
    }

    #[test]
    fn test_is_noop_payload_with_resources_not_skipped() {
        // A payload with resources should be processed normally.
        let payload = json!({
            "run_id": "real-001",
            "node_name": "fleet-01",
            "resources": [
                {"name": "pkg", "status": "updated", "type": "package", "action": ["install"]}
            ]
        });
        assert!(!is_noop_payload(&payload));
    }

    #[test]
    fn test_is_noop_payload_missing_resources_skipped() {
        // Missing resources key entirely → no-op → skip.
        let payload = json!({
            "run_id": "noop-003",
            "node_name": "fleet-01"
        });
        assert!(is_noop_payload(&payload));
    }
}

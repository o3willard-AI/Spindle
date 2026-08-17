//! S-16: Worker integration tests
//!
//! Exercises the pipeline worker end-to-end against a real PostgreSQL database.
//! Each test:
//! 1. Spins up a DB connection (skips if DB unavailable)
//! 2. Enqueues a job with a test payload in the archive
//! 3. Runs the worker's `process_one()` method
//! 4. Asserts the resulting state in the store tables
//!
//! Tests cover:
//! - Happy path: dequeue → parse → store
//! - No-op filtering (up-to-date resources skipped)
//! - Compliance report processing
//! - Dead-letter queue (malformed → DLQ)
//! - DLQ retry (re-enqueue → still fails → permanent DLQ)
//! - Multiple jobs in queue (sequential processing)
//! - Schema version stamping
//! - Cookbook usage tracking
//! - Duration rollup aggregation

#![allow(warnings)]
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use spindle_rawarchive::{Archive as ArchiveTrait, ArchiveMetadata, LocalArchive};
use spindle_worker::{PipelineWorker, WorkerConfig};

/// Live PostgreSQL connection string.
/// Override with DATABASE_URL env var for testing against a fresh scratch DB.
fn db_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://spindle:CHANGE_ME@192.0.2.10:5432/spindle".to_string()
    })
}
const TEST_ARCHIVE_DIR: &str = "/tmp/spindle-worker-tests";

/// Generate a short unique ID for test names.
fn short_id() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

/// Try to connect to the live database. Returns None if unavailable.
async fn try_db_pool() -> Option<PgPool> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url())
        .await
        .ok()
}

/// Create a worker for testing with a given pool and archive dir.
fn make_worker(pool: PgPool) -> PipelineWorker {
    PipelineWorker::new_with_pool(pool, TEST_ARCHIVE_DIR, Duration::from_secs(0))
        .expect("failed to create worker")
}

/// Ensure the test archive directory exists.
fn ensure_archive_dir() {
    std::fs::create_dir_all(TEST_ARCHIVE_DIR).ok();
}

/// Build a key for an archive payload (date-based, like the real worker).
fn build_archive_key() -> String {
    let now = chrono::Utc::now();
    format!("{}", now.format("%Y/%m/%d"))
}

/// Archive a JSON payload and return its key.
fn archive_payload(payload: &serde_json::Value) -> String {
    ensure_archive_dir();
    let bytes = serde_json::to_vec(payload).unwrap();
    let digest = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(&bytes);
        hex::encode(h.finalize())
    };
    let metadata = ArchiveMetadata::new(
        digest,
        "application/json".to_string(),
        "test-token".to_string(),
        chrono::Utc::now(),
    );
    let archive = LocalArchive::new(TEST_ARCHIVE_DIR).expect("failed to open archive");
    ArchiveTrait::store(&archive, &bytes, &metadata).expect("failed to archive")
}

/// Archive raw bytes and return their key.
fn archive_raw_bytes(raw: &[u8]) -> String {
    ensure_archive_dir();
    let digest = format!("malformed-{}", Uuid::new_v4().to_string()[..8].to_string());
    let metadata = ArchiveMetadata::new(
        digest,
        "application/json".to_string(),
        "test-token".to_string(),
        chrono::Utc::now(),
    );
    let archive = LocalArchive::new(TEST_ARCHIVE_DIR).expect("failed to open archive");
    // For malformed payloads, we need to write raw bytes directly to storage
    // since the Archive::store method gzips the payload, and the worker's
    // retrieve() will decompress. For malformed JSON, we still need the
    // archive to return the raw bytes properly. Use storage().put directly.
    let key = format!(
        "{}/{}.json",
        build_archive_key(),
        Uuid::new_v4().to_string()
    );
    ArchiveTrait::store(&archive, raw, &metadata).expect("failed to archive")
}

/// Clean up test data in the database.
async fn cleanup_test_data(pool: &PgPool) {
    let _ = sqlx::query(
        "DELETE FROM resource_events WHERE run_id IN \
         (SELECT id FROM runs WHERE node_id IN \
         (SELECT id FROM nodes WHERE name LIKE 'worker-test-%'))",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM runs WHERE node_id IN \
         (SELECT id FROM nodes WHERE name LIKE 'worker-test-%')",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM nodes WHERE name LIKE 'worker-test-%'")
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM jobs WHERE node_name LIKE 'worker-test-%'")
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM pipeline_dead_letter WHERE node_name LIKE 'worker-test-%'")
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM cookbook_usage WHERE node_id IN \
         (SELECT id FROM nodes WHERE name LIKE 'worker-test-%')",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM compliance_reports WHERE node_id IN \
         (SELECT id FROM nodes WHERE name LIKE 'worker-test-%')",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM control_results WHERE node_id IN \
         (SELECT id FROM nodes WHERE name LIKE 'worker-test-%')",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM profiles WHERE name LIKE 'worker-test-%'")
        .execute(pool)
        .await;
}

/// Enqueue a job in the database and return its ID.
async fn enqueue_job(
    pool: &PgPool,
    payload_key: &str,
    node_name: &str,
    max_retries: i32,
) -> String {
    let job_id = format!(
        "worker-test-{}-{}",
        short_id(),
        Uuid::new_v4().to_string()[..8].to_string()
    );
    sqlx::query(
        r#"
        INSERT INTO jobs (id, payload_key, node_id, node_name, run_id, status, retry_count, max_retries, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, 'pending', 0, $6, NOW(), NOW())
        "#,
    )
    .bind(&job_id)
    .bind(payload_key)
    .bind(Uuid::new_v4().to_string())
    .bind(node_name)
    .bind(format!("run-{}", short_id()))
    .bind(max_retries)
    .execute(pool)
    .await
    .expect("failed to enqueue job");
    job_id
}

/// Build a minimal valid run-converge payload with the given resource events.
fn make_run_converge_payload(
    node_name: &str,
    resources: Vec<serde_json::Value>,
) -> serde_json::Value {
    json!({
        "run_id": format!("run-{}", short_id()),
        "node_name": node_name,
        "status": "success",
        "start_time": "2024-01-01T00:00:00Z",
        "end_time": "2024-01-01T00:00:01Z",
        "total_resource_count": resources.len(),
        "updated_count": resources.len(),
        "node": {
            "name": node_name,
            "chef_environment": "production",
            "automatic": {
                "platform": "ubuntu",
                "platform_version": "22.04"
            }
        },
        "policy_group": "web",
        "policy_name": "apache2",
        "resources": resources,
    })
}

/// Build a resource event JSON object.
fn make_resource(
    name: &str,
    status: &str,
    cookbook: &str,
    version: &str,
    duration: f64,
) -> serde_json::Value {
    json!({
        "name": name,
        "type": "package",
        "status": status,
        "action": ["install"],
        "duration": duration,
        "cookbook_name": cookbook,
        "cookbook_version": version,
        "delta": "",
    })
}

/// Build a minimal valid Cinc Auditor compliance report payload.
fn make_auditor_payload(node_name: &str) -> serde_json::Value {
    json!({
        "platform": {
            "name": node_name,
            "release": "1.0.0",
        },
        "profiles": [
            {
                "name": format!("worker-test-profile-{}", short_id()),
                "version": "1.0.0",
                "title": "Worker Test Profile",
                "sha256": "abc123",
                "controls": [
                    {
                        "id": "worker-test-control-1",
                        "title": "Test Control 1",
                        "desc": "Ensure something",
                        "impact": 1.0,
                        "tags": {},
                        "refs": [],
                        "source_location": {},
                        "code": "",
                        "results": [
                            {
                                "status": "passed",
                                "code": "",
                                "run_time": 0.1,
                                "start_time": "2024-01-01T00:00:00Z",
                                "message": null,
                                "skip_reason": null,
                            },
                        ],
                    },
                    {
                        "id": "worker-test-control-2",
                        "title": "Test Control 2",
                        "desc": "Ensure something else",
                        "impact": 0.5,
                        "tags": {},
                        "refs": [],
                        "source_location": {},
                        "code": "",
                        "results": [
                            {
                                "status": "failed",
                                "code": "",
                                "run_time": 0.2,
                                "start_time": "2024-01-01T00:00:01Z",
                                "message": "Failed check",
                                "skip_reason": null,
                            },
                        ],
                    },
                ],
            },
        ],
        "statistics": {
            "duration": 1.0,
        },
    })
}

/// Helper: skip test if DB is unavailable.
async fn setup() -> Option<(PgPool, PipelineWorker)> {
    ensure_archive_dir();
    let pool = try_db_pool().await?;
    if pool.is_closed() {
        return None;
    }
    // Verify DB connectivity
    let _ = sqlx::query("SELECT 1 FROM jobs LIMIT 1")
        .fetch_optional(&pool)
        .await;
    let worker = make_worker(pool.clone());
    Some((pool, worker))
}

// ─── Test 1: Happy path — dequeue → parse → store ──────────────────────────

#[tokio::test]
async fn test_worker_dequeue_parse_store_happy_path() {
    let (pool, worker) = match setup().await {
        Some(x) => x,
        None => {
            eprintln!("SKIP: DB unavailable");
            return;
        }
    };

    let node_name = format!("worker-test-happy-{}", short_id());
    let payload = make_run_converge_payload(
        &node_name,
        vec![
            make_resource("apache2", "updated", "apache2", "1.0.0", 1.5),
            make_resource("curl", "updated", "curl", "2.0.0", 0.3),
        ],
    );
    let payload_key = archive_payload(&payload);
    let job_id = enqueue_job(&pool, &payload_key, &node_name, 3).await;

    let result = worker.process_one().await;
    assert!(result.is_ok(), "process_one failed: {:?}", result.err());
    let processed = result.unwrap();
    assert!(processed.is_some(), "no job was dequeued");
    assert_eq!(processed.unwrap().id, job_id);

    // Verify node was stored
    let node_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE name = $1")
        .bind(&node_name)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(node_count >= 1, "node was not stored");

    // Verify run was stored
    let run_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runs WHERE node_id IN (SELECT id FROM nodes WHERE name = $1)",
    )
    .bind(&node_name)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(run_count >= 1, "run was not stored");

    // Verify resource events were stored (2 updated events)
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM resource_events WHERE node_id IN (SELECT id FROM nodes WHERE name = $1)"
    )
    .bind(&node_name)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        event_count, 2,
        "expected 2 resource events, got {}",
        event_count
    );

    // Verify job is completed
    let job_status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
        .bind(&job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(job_status, "completed", "job should be completed");

    cleanup_test_data(&pool).await;
}

// ─── Test 2: No-op filtering ────────────────────────────────────────────────

#[tokio::test]
async fn test_worker_noop_filtering_skips_noop_converge() {
    let (pool, worker) = match setup().await {
        Some(x) => x,
        None => {
            eprintln!("SKIP: DB unavailable");
            return;
        }
    };

    let node_name = format!("worker-test-noop-{}", short_id());
    // Empty resources array → no-op
    let payload = make_run_converge_payload(&node_name, vec![]);
    let payload_key = archive_payload(&payload);
    let job_id = enqueue_job(&pool, &payload_key, &node_name, 3).await;

    let result = worker.process_one().await;
    assert!(result.is_ok(), "process_one failed: {:?}", result.err());

    // Verify job is skipped (not completed, not dead_lettered)
    let job_status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
        .bind(&job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        job_status, "skipped",
        "no-op job should be skipped, got {}",
        job_status
    );

    // Verify no resource events were stored
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM resource_events WHERE node_id IN (SELECT id FROM nodes WHERE name = $1)"
    )
    .bind(&node_name)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        event_count, 0,
        "no resource events should be stored for no-op"
    );

    cleanup_test_data(&pool).await;
}

// ─── Test 3: Compliance report processing ────────────────────────────────────

#[tokio::test]
async fn test_worker_compliance_report_processing() {
    let (pool, worker) = match setup().await {
        Some(x) => x,
        None => {
            eprintln!("SKIP: DB unavailable");
            return;
        }
    };

    let node_name = format!("worker-test-comp-{}", short_id());
    let payload = make_auditor_payload(&node_name);
    let payload_key = archive_payload(&payload);
    let job_id = enqueue_job(&pool, &payload_key, &node_name, 3).await;

    let result = worker.process_one().await;
    assert!(result.is_ok(), "process_one failed: {:?}", result.err());

    // Verify job is completed
    let job_status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
        .bind(&job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        job_status, "completed",
        "compliance job should be completed"
    );

    // Verify profiles were stored
    let profile_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM profiles WHERE name LIKE 'worker-test-%'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(profile_count >= 1, "at least 1 profile should be stored");

    // Verify compliance reports were stored
    let report_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM compliance_reports WHERE node_id IN (SELECT id FROM nodes WHERE name = $1)"
    )
    .bind(&node_name)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        report_count >= 1,
        "at least 1 compliance report should be stored"
    );

    // Verify control results were stored (2 controls in our test payload)
    let ctrl_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM control_results WHERE node_id IN (SELECT id FROM nodes WHERE name = $1)"
    )
    .bind(&node_name)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        ctrl_count >= 2,
        "expected at least 2 control results, got {}",
        ctrl_count
    );

    cleanup_test_data(&pool).await;
}

// ─── Test 4: Dead-letter queue (malformed → DLQ) ────────────────────────────

#[tokio::test]
async fn test_worker_dead_letter_malformed_payload() {
    let (pool, worker) = match setup().await {
        Some(x) => x,
        None => {
            eprintln!("SKIP: DB unavailable");
            return;
        }
    };

    let node_name = format!("worker-test-dlq-{}", short_id());
    let malformed_bytes = b"not valid json {{{";
    let payload_key = archive_raw_bytes(malformed_bytes);
    let job_id = enqueue_job(&pool, &payload_key, &node_name, 1).await;

    let result = worker.process_one().await;
    assert!(
        result.is_ok(),
        "process_one should succeed (handles failure internally)"
    );
    assert!(result.unwrap().is_some(), "a job should have been dequeued");

    // With max_retries=1, new_retry_count (1) >= max_retries (1) → dead_lettered
    let job_status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
        .bind(&job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        job_status, "dead_lettered",
        "malformed job should be dead-lettered, got {}",
        job_status
    );

    // Verify dead letter entry exists
    let dl_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pipeline_dead_letter WHERE archive_reference = $1",
    )
    .bind(&payload_key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(dl_count >= 1, "dead letter entry should exist");

    cleanup_test_data(&pool).await;
}

// ─── Test 5: DLQ retry → permanent ───────────────────────────────────────────

#[tokio::test]
async fn test_worker_dlq_retry_then_permanent() {
    let (pool, worker) = match setup().await {
        Some(x) => x,
        None => {
            eprintln!("SKIP: DB unavailable");
            return;
        }
    };

    let node_name = format!("worker-test-retry-{}", short_id());
    let malformed_bytes = b"not valid json {{{";
    let payload_key = archive_raw_bytes(malformed_bytes);
    let job_id = enqueue_job(&pool, &payload_key, &node_name, 3).await;

    // First attempt: fails, re-queued with retry_count=1
    worker
        .process_one()
        .await
        .expect("process_one should not error");
    let job_status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
        .bind(&job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        job_status, "pending",
        "after first failure, job should be re-queued (pending)"
    );

    let retry_count: i32 = sqlx::query_scalar("SELECT retry_count FROM jobs WHERE id = $1")
        .bind(&job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        retry_count, 1,
        "retry_count should be 1 after first failure"
    );

    // Second attempt: fails, re-queued with retry_count=2
    worker
        .process_one()
        .await
        .expect("process_one should not error");
    let retry_count: i32 = sqlx::query_scalar("SELECT retry_count FROM jobs WHERE id = $1")
        .bind(&job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        retry_count, 2,
        "retry_count should be 2 after second failure"
    );

    // Third attempt: fails, retry_count=3 >= max_retries=3 → dead_lettered
    worker
        .process_one()
        .await
        .expect("process_one should not error");
    let job_status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
        .bind(&job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        job_status, "dead_lettered",
        "after exhausting retries, job should be dead-lettered"
    );

    cleanup_test_data(&pool).await;
}

// ─── Test 6: Multiple jobs sequential ────────────────────────────────────────

#[tokio::test]
async fn test_worker_multiple_jobs_sequential() {
    let (pool, worker) = match setup().await {
        Some(x) => x,
        None => {
            eprintln!("SKIP: DB unavailable");
            return;
        }
    };

    let base_name = format!("worker-test-multi-{}", short_id());

    let mut job_ids = Vec::new();
    for i in 0..3 {
        let node_name = format!("{}-{}", base_name, i);
        let payload = make_run_converge_payload(
            &node_name,
            vec![make_resource(
                "pkg-a",
                "updated",
                "cookbook-a",
                "1.0.0",
                1.0,
            )],
        );
        let payload_key = archive_payload(&payload);
        let job_id = enqueue_job(&pool, &payload_key, &node_name, 3).await;
        job_ids.push(job_id);
    }

    // Process all 3 jobs sequentially
    for i in 0..3 {
        let result = worker.process_one().await;
        assert!(
            result.is_ok(),
            "process_one failed on iteration {}: {:?}",
            i,
            result.err()
        );
        assert!(
            result.unwrap().is_some(),
            "job {} should have been dequeued",
            i
        );
    }

    // Verify all 3 jobs are completed
    for job_id in &job_ids {
        let status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            status, "completed",
            "job {} should be completed, got {}",
            job_id, status
        );
    }

    // Verify 4th dequeue returns no job
    let result = worker.process_one().await;
    assert!(result.is_ok());
    assert!(
        result.unwrap().is_none(),
        "no more jobs should be available"
    );

    cleanup_test_data(&pool).await;
}

// ─── Test 7: Schema version stamping ────────────────────────────────────────

#[tokio::test]
async fn test_worker_schema_version_stamping() {
    let (pool, worker) = match setup().await {
        Some(x) => x,
        None => {
            eprintln!("SKIP: DB unavailable");
            return;
        }
    };

    let node_name = format!("worker-test-schema-{}", short_id());
    let payload = make_run_converge_payload(
        &node_name,
        vec![make_resource("pkg", "updated", "cookbook", "1.0.0", 1.0)],
    );
    let payload_key = archive_payload(&payload);
    let job_id = enqueue_job(&pool, &payload_key, &node_name, 3).await;

    worker.process_one().await.expect("process_one failed");

    // Verify resource_events have schema_version stamped
    let events_with_version: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM resource_events 
           WHERE node_id IN (SELECT id FROM nodes WHERE name = $1)
           AND schema_version = $2"#,
    )
    .bind(&node_name)
    .bind(spindle_pipeline::SCHEMA_VERSION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        events_with_version >= 1,
        "resource events should have schema_version stamped"
    );

    // Verify runs have schema_version stamped
    let runs_with_version: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM runs 
           WHERE node_id IN (SELECT id FROM nodes WHERE name = $1)
           AND schema_version = $2"#,
    )
    .bind(&node_name)
    .bind(spindle_pipeline::SCHEMA_VERSION)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        runs_with_version >= 1,
        "runs should have schema_version stamped"
    );

    let _ = job_id;
    cleanup_test_data(&pool).await;
}

// ─── Test 8: Cookbook usage tracking ────────────────────────────────────────

#[tokio::test]
async fn test_worker_cookbook_usage_tracking() {
    let (pool, worker) = match setup().await {
        Some(x) => x,
        None => {
            eprintln!("SKIP: DB unavailable");
            return;
        }
    };

    let node_name = format!("worker-test-cb-{}", short_id());
    let payload = make_run_converge_payload(
        &node_name,
        vec![
            make_resource("pkg-a", "updated", "apache2", "1.0.0", 1.5),
            make_resource("pkg-b", "updated", "apache2", "1.0.0", 0.3), // same cookbook, same version
            make_resource("pkg-c", "updated", "curl", "2.0.0", 0.1),
        ],
    );
    let payload_key = archive_payload(&payload);
    let job_id = enqueue_job(&pool, &payload_key, &node_name, 3).await;

    worker.process_one().await.expect("process_one failed");

    // Verify cookbook_usage entries exist for our cookbooks
    let usage_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM cookbook_usage 
           WHERE cookbook_name IN ('apache2', 'curl')
           AND node_id IN (SELECT id FROM nodes WHERE name = $1)"#,
    )
    .bind(&node_name)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        usage_count >= 2,
        "expected at least 2 cookbook usage entries, got {}",
        usage_count
    );

    let _ = job_id;
    cleanup_test_data(&pool).await;
}

// ─── Test 9: Duration rollup aggregation ────────────────────────────────────

#[tokio::test]
async fn test_worker_duration_rollup_aggregation() {
    let (pool, worker) = match setup().await {
        Some(x) => x,
        None => {
            eprintln!("SKIP: DB unavailable");
            return;
        }
    };

    let node_name = format!("worker-test-rollup-{}", short_id());
    let payload = make_run_converge_payload(
        &node_name,
        vec![
            make_resource("pkg-a", "updated", "cookbook-a", "1.0.0", 2.5),
            make_resource("pkg-b", "failed", "cookbook-b", "2.0.0", 1.3),
            make_resource("pkg-c", "skipped", "cookbook-c", "3.0.0", 0.5),
        ],
    );
    let payload_key = archive_payload(&payload);
    let job_id = enqueue_job(&pool, &payload_key, &node_name, 3).await;

    worker.process_one().await.expect("process_one failed");

    // Verify runs have correct stats
    let run_row: (String, Option<String>, i32, i32, i32, i32) = sqlx::query_as(
        r#"SELECT status, end_time, total_resource_count, updated_count, failed_count, skipped_count
           FROM runs WHERE node_id IN (SELECT id FROM nodes WHERE name = $1)
           ORDER BY created_at DESC LIMIT 1"#,
    )
    .bind(&node_name)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(run_row.0, "success", "run status should be success");
    assert_eq!(run_row.2, 3, "total_resource_count should be 3");
    assert_eq!(run_row.3, 1, "updated_count should be 1");
    assert_eq!(run_row.4, 1, "failed_count should be 1");
    assert_eq!(run_row.5, 1, "skipped_count should be 1");

    // Verify resource events have duration_ms populated
    let total_duration: i64 = sqlx::query_scalar(
        r#"SELECT COALESCE(SUM(duration_ms), 0) 
           FROM resource_events WHERE node_id IN (SELECT id FROM nodes WHERE name = $1)"#,
    )
    .bind(&node_name)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        total_duration > 0,
        "resource events should have duration data"
    );

    let _ = job_id;
    cleanup_test_data(&pool).await;
}

// ─── Test 5: Auditor node dedup — same payload twice → 1 node row ──────────

#[tokio::test]
async fn test_auditor_node_dedup_no_duplicates() {
    let (pool, worker) = match setup().await {
        Some(x) => x,
        None => {
            eprintln!("SKIP: DB unavailable");
            return;
        }
    };

    let node_name = format!("worker-test-dedup-{}", short_id());
    let payload = make_auditor_payload(&node_name);

    // Enqueue the same auditor payload twice — simulating the real ingest
    // handler which generates a fresh random node_id per scan (every 2 min).
    // Each enqueue gets a different random UUID for node_id, exactly as the
    // auditor_handler does in ingest.rs.
    let payload_key_1 = archive_payload(&payload);
    let _job_id_1 = enqueue_job(&pool, &payload_key_1, &node_name, 3).await;

    let result = worker.process_one().await;
    assert!(result.is_ok(), "first process_one failed: {:?}", result.err());

    // Enqueue the same payload again with a different job/node_id
    let payload_key_2 = archive_payload(&payload);
    let _job_id_2 = enqueue_job(&pool, &payload_key_2, &node_name, 3).await;

    let result = worker.process_one().await;
    assert!(result.is_ok(), "second process_one failed: {:?}", result.err());

    // CRITICAL ASSERTION: exactly 1 node row for this name, not 2.
    // Before the fix, each scan inserted a duplicate row.
    let node_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE name = $1")
        .bind(&node_name)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        node_count, 1,
        "expected exactly 1 node row for '{}' after 2 auditor scans, got {} — duplicate nodes created",
        node_name, node_count
    );

    // Verify last_seen was updated (not just the original row left stale)
    let last_seen_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM nodes WHERE name = $1 AND last_seen > NOW() - INTERVAL '10 seconds'",
    )
    .bind(&node_name)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        last_seen_count, 1,
        "node last_seen should have been updated by the second scan"
    );

    cleanup_test_data(&pool).await;
}

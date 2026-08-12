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

#![allow(warnings)]
use std::time::Duration;

use tracing::{error, info, warn};

use spindle_worker::{PipelineWorker, WorkerConfig, CLAIM_TIMEOUT, RECOVERY_INTERVAL, SHUTDOWN_DEADLINE};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing with three-tier logging support
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

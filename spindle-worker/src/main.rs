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
//!
//! `--version` / `-V` prints the build info and exits immediately (no DB connection).

#![allow(warnings)]
use std::time::Duration;

use tracing::{error, info, warn};

use spindle_worker::{
    PipelineWorker, WorkerConfig, CLAIM_TIMEOUT, RECOVERY_INTERVAL, SHUTDOWN_DEADLINE,
};

/// Build info: git commit SHA (short) and build date, set by build.rs.
const GIT_SHA: &str = env!("SPINDLE_GIT_SHA");
const BUILD_DATE: &str = env!("SPINDLE_BUILD_DATE");

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Handle --version / -V BEFORE any config/obs/DB initialization.
    // --version must never open a DB connection or hang on init.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!(
            "spindle-worker {} (git: {}, built: {})",
            env!("CARGO_PKG_VERSION"),
            GIT_SHA,
            BUILD_DATE,
        );
        std::process::exit(0);
    }

    // Initialize observability via spindle-obs (single source of truth)
    let obs_config = spindle_obs::Config::from_env("operational");
    spindle_obs::init(&obs_config);

    let config = WorkerConfig::from_env();
    // Validate config
    if let Err(e) = spindle_config::Config::load() {
        eprintln!("Config load warning: {}", e);
    }

    let worker = PipelineWorker::new(config).await?;
    worker.run().await?;

    Ok(())
}

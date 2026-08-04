//! Spindle Observability — request tracing + secret scanning.
//!
//! # Usage
//! ```ignore
//! spindle_obs::init("production"); // sets up tracing + middleware
//! ```

use std::sync::atomic::{AtomicBool, Ordering};

/// Configuration for the observability subsystem.
#[derive(Debug, Clone)]
pub struct Config {
    /// Log level: "trace", "debug", "info", "warn", "error".
    pub level: String,
    /// Target: "stdout" (TTY) or "json".
    pub target: String,
    /// Whether to enable secret scanning on log lines.
    pub scan_secrets: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            target: "stdout".to_string(),
            scan_secrets: true,
        }
    }
}

/// Shared state: whether observability has been initialized.
static INITED: AtomicBool = AtomicBool::new(false);

/// Install the observability subsystem.
///
/// Sets up `tracing-subscriber` (JSON or text) and installs the
/// `X-Request-Id` middleware on the axum `ServiceExt`.
pub fn init(cfg: &Config) {
    let cfg = Config {
        scan_secrets: cfg.scan_secrets && cfg.target == "stdout",
        ..cfg.clone()
    };

    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_env_filter(
            tracing_subscriber::EnvFilter::new(&cfg.level),
        )
        .json()
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("tracing subscriber already set");

    INITED.store(true, Ordering::Relaxed);
    tracing::info!("spindle-obs initialized: level={}, target={}", cfg.level, cfg.target);
}

/// Check whether observability has been initialized.
pub fn is_initialized() -> bool {
    INITED.load(Ordering::Relaxed)
}

// ──────────────────────────────────────────────────────────────────────
// Request ID generation
// ──────────────────────────────────────────────────────────────────────

/// Generate a new request identifier (UUIDv7 — timestamp-first, sortable).
pub fn generate_request_id() -> String {
    crate::request_id::generate_request_id().to_string()
}

// ──────────────────────────────────────────────────────────────────────
// Middleware
// ──────────────────────────────────────────────────────────────────────

mod middleware;

mod request_id;

// ──────────────────────────────────────────────────────────────────────
// Secret scanning
// ──────────────────────────────────────────────────────────────────────

mod secret_scan;

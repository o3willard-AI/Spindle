//! Spindle Observability — request tracing + secret scanning.
//!
//! # Usage
//! ```ignore
//! let obs_cfg = spindle_obs::Config {
//!     log_level: "debug".to_string(), // L1=info, L2=debug, L3=trace
//!     ..Default::default()
//! };
//! spindle_obs::init(&obs_cfg);
//! ```

#![allow(warnings)]
use std::sync::atomic::{AtomicBool, Ordering};

/// Configuration for the observability subsystem.
#[derive(Debug, Clone)]
pub struct Config {
    /// Log level: "trace", "debug", "info", "warn", "error".
    /// Maps to three-tier logging:
    /// - L1 Operational = "info"
    /// - L2 Diagnostic  = "debug"
    /// - L3 Debug        = "trace"
    pub level: String,
    /// Target: "stdout" (TTY) or "json".
    pub target: String,
    /// Whether to enable secret scanning on log lines.
    pub scan_secrets: bool,
    /// Three-tier log level: "operational" (L1), "diagnostic" (L2), "debug" (L3).
    /// Equivalent to level but uses the spec's tier names. Takes priority over `level`
    /// if both are set. Allows per-crate overrides via RUST_LOG.
    pub log_level: Option<LogLevel>,
}

/// Three-tier log level mapping (per docs/logging-architecture.md).
/// - L1 Operational → info
/// - L2 Diagnostic  → debug
/// - L3 Debug        → trace
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Operational, // L1 → info
    Diagnostic,  // L2 → debug
    Debug,       // L3 → trace
}

impl std::str::FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "operational" | "l1" | "info" => Ok(LogLevel::Operational),
            "diagnostic" | "l2" | "debug" => Ok(LogLevel::Diagnostic),
            "debug" | "l3" | "trace" => Ok(LogLevel::Debug),
            _ => Err(format!(
                "invalid log_level '{}': expected 'operational', 'diagnostic', or 'debug'",
                s
            )),
        }
    }
}

impl LogLevel {
    /// Convert to the tracing level string used by EnvFilter.
    pub fn as_tracing_level(&self) -> &'static str {
        match self {
            LogLevel::Operational => "info",
            LogLevel::Diagnostic => "debug",
            LogLevel::Debug => "trace",
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            target: "stdout".to_string(),
            scan_secrets: true,
            log_level: Some(LogLevel::Operational), // L1 by default
        }
    }
}

impl Config {
    /// Build a Config from a tier string ("operational", "diagnostic", "debug").
    pub fn from_tier(tier: &str) -> Self {
        let level = match tier.to_lowercase().as_str() {
            "operational" | "info" => "info",
            "diagnostic" | "debug" => "debug",
            "trace" => "trace",
            _ => "info",
        };
        Self {
            level: level.to_string(),
            target: "json".to_string(),
            scan_secrets: false,
            log_level: Some(tier.parse().unwrap_or(LogLevel::Operational)),
        }
    }

    /// Build a Config from a tracing level string ("info", "debug", "trace").
    pub fn from_level(level: &str) -> Self {
        let log_level = match level.to_lowercase().as_str() {
            "trace" => LogLevel::Debug,
            "debug" => LogLevel::Diagnostic,
            "info" | "warn" | "error" => LogLevel::Operational,
            _ => LogLevel::Operational,
        };
        Self {
            level: level.to_string(),
            target: "json".to_string(),
            scan_secrets: false,
            log_level: Some(log_level),
        }
    }
}

/// Shared state: whether observability has been initialized.
static INITED: AtomicBool = AtomicBool::new(false);

/// Install the observability subsystem.
///
/// Sets up `tracing-subscriber` (JSON or text) and installs the
/// `X-Request-Id` middleware on the axum `ServiceExt`.
/// The `log_level` field maps three-tier names to tracing levels:
/// Operational→info, Diagnostic→debug, Debug→trace.
/// If `log_level` is None, falls back to `level` field.
/// Supports per-crate overrides via RUST_LOG (e.g.
/// `RUST_LOG=spindle_worker=debug,spindle_pipeline=trace`).
pub fn init(cfg: &Config) {
    let effective_level = match cfg.log_level {
        Some(ref tier) => tier.as_tracing_level(),
        None => cfg.level.as_str(),
    };

    let cfg = Config {
        scan_secrets: cfg.scan_secrets && cfg.target == "stdout",
        ..cfg.clone()
    };

    // Build EnvFilter: use RUST_LOG if set (allows per-crate overrides),
    // otherwise use the tier-mapped level as default.
    let env_filter = match std::env::var("RUST_LOG") {
        Ok(rust_log) => tracing_subscriber::EnvFilter::new(&rust_log),
        Err(_) => tracing_subscriber::EnvFilter::new(effective_level),
    };

    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_env_filter(env_filter)
        .json()
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("tracing subscriber already set");

    INITED.store(true, Ordering::Relaxed);
    tracing::info!(
        level = %effective_level,
        target = %cfg.target,
        scan_secrets = cfg.scan_secrets,
        "spindle-obs initialized"
    );
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

pub mod middleware;
pub mod request_id;
pub mod secret_scan;

// Re-export commonly used items
pub use middleware::request_id_middleware;
pub use request_id::generate_request_id as new_request_id;
pub use secret_scan::{scan_log_line, ScanResult};

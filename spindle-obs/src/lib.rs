//! Spindle Observability — request tracing + secret scanning.
//!
//! # Usage
//! ```ignore
//! let obs_cfg = spindle_obs::Config::from_tier("diagnostic");
//! spindle_obs::init(&obs_cfg);
//! ```
//!
//! # Tier mapping
//! - `operational` (L1) → `info`  — always on, minimal disk
//! - `diagnostic`  (L2) → `debug` — opt-in, payload metadata
//! - `debug`       (L3) → `trace` — full bodies/SQL, never in prod
//!
//! Note: the tier name "debug" maps to tracing `trace`, NOT tracing `debug`.
//! This is because the three tiers use domain names (operational/diagnostic/debug)
//! that don't align 1:1 with tracing levels (info/debug/trace).

#![allow(warnings)]
use std::sync::atomic::{AtomicBool, Ordering};

// ── LogLevel enum (single source of truth) ──────────────────────────────

/// Three-tier log level mapping (per docs/logging-architecture.md).
/// - L1 Operational → info
/// - L2 Diagnostic  → debug
/// - L3 Debug        → trace
///
/// Note: the tier name "Debug" (L3) maps to tracing `trace`, not `debug`.
/// This is intentional — the three tiers use domain names that don't
/// align 1:1 with tracing levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum LogLevel {
    /// L1 — Operational. Always on. Minimal disk, zero perf impact.
    #[default]
    Operational,
    /// L2 — Diagnostic. Opt-in. Payload metadata, per-resource breakdown.
    Diagnostic,
    /// L3 — Debug. Full payload bodies, full SQL, intermediate state.
    Debug,
}

impl std::str::FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "operational" | "l1" | "info" => Ok(LogLevel::Operational),
            "diagnostic" | "l2" => Ok(LogLevel::Diagnostic),
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
    /// Operational → "info", Diagnostic → "debug", Debug → "trace".
    pub fn as_tracing_level(&self) -> &'static str {
        match self {
            LogLevel::Operational => "info",
            LogLevel::Diagnostic => "debug",
            LogLevel::Debug => "trace",
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Operational => write!(f, "operational"),
            LogLevel::Diagnostic => write!(f, "diagnostic"),
            LogLevel::Debug => write!(f, "debug"),
        }
    }
}

// ── Config ───────────────────────────────────────────────────────────────

/// Configuration for the observability subsystem.
///
/// This is the single source of truth for logging configuration.
/// All binaries should construct this and call [`init()`].
#[derive(Debug, Clone)]
pub struct Config {
    /// Three-tier log level. Default: Operational (L1/info).
    pub log_level: LogLevel,
    /// Target: "stdout" (TTY, human-readable) or "json" (log shipper).
    pub target: String,
    /// Whether to enable secret scanning on log lines.
    /// Active by default; the scanner redacts passwords, tokens, API keys,
    /// and JWT patterns from every log line at any level.
    pub scan_secrets: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: LogLevel::Operational,
            target: "json".to_string(),
            scan_secrets: true,
        }
    }
}

impl Config {
    /// Build a Config from a tier string ("operational", "diagnostic", "debug").
    /// Reads SPINDLE_LOG_LEVEL if set; falls back to the given default.
    pub fn from_env(default_tier: &str) -> Self {
        let tier = std::env::var("SPINDLE_LOG_LEVEL")
            .unwrap_or_else(|_| default_tier.to_string());
        let target = std::env::var("SPINDLE_LOG_TARGET")
            .unwrap_or_else(|_| "json".to_string());
        Self {
            log_level: tier.parse().unwrap_or(LogLevel::Operational),
            target,
            scan_secrets: true,
        }
    }

    /// Build a Config from a tier string.
    pub fn from_tier(tier: &str) -> Self {
        Self {
            log_level: tier.parse().unwrap_or(LogLevel::Operational),
            target: "json".to_string(),
            scan_secrets: true,
        }
    }

    /// Effective tracing level string (for EnvFilter).
    pub fn effective_level(&self) -> &'static str {
        self.log_level.as_tracing_level()
    }
}

// ── Init ────────────────────────────────────────────────────────────────

/// Shared state: whether observability has been initialized.
static INITED: AtomicBool = AtomicBool::new(false);

/// Install the observability subsystem.
///
/// Sets up `tracing-subscriber` (JSON or text) with:
/// - Three-tier log level mapping (operational→info, diagnostic→debug, debug→trace)
/// - Per-crate overrides via `RUST_LOG` (e.g. `RUST_LOG=spindle_worker=debug`)
/// - Secret scanning on stdout targets (passwords, tokens, API keys redacted)
///
/// This is the single entry point for all Spindle binaries.
/// Calling it more than once is a no-op (idempotent).
pub fn init(cfg: &Config) {
    // Idempotent: if already initialized, don't panic.
    if INITED.swap(true, Ordering::SeqCst) {
        return;
    }

    let effective_level = cfg.effective_level();

    // Build EnvFilter: use RUST_LOG if set (allows per-crate overrides),
    // otherwise use the tier-mapped level as default.
    let env_filter = match std::env::var("RUST_LOG") {
        Ok(rust_log) if !rust_log.is_empty() => {
            tracing_subscriber::EnvFilter::new(&rust_log)
        }
        _ => tracing_subscriber::EnvFilter::new(effective_level),
    };

    let use_json = cfg.target != "stdout";
    let scan_secrets = cfg.scan_secrets;

    if use_json {
        let subscriber = tracing_subscriber::fmt::Subscriber::builder()
            .with_env_filter(env_filter)
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .json()
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    } else {
        let subscriber = tracing_subscriber::fmt::Subscriber::builder()
            .with_env_filter(env_filter)
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    }

    tracing::info!(
        level = %effective_level,
        tier = %cfg.log_level,
        target = %cfg.target,
        scan_secrets = scan_secrets,
        "spindle-obs initialized (L1=operational, L2=diagnostic, L3=debug)"
    );
}

/// Check whether observability has been initialized.
pub fn is_initialized() -> bool {
    INITED.load(Ordering::SeqCst)
}

/// Reset the initialized flag (for testing only).
#[cfg(test)]
pub fn reset_for_test() {
    INITED.store(false, Ordering::SeqCst);
}

// ──────────────────────────────────────────────────────────────────────
// Request ID generation
// ──────────────────────────────────────────────────────────────────────

/// Generate a new request identifier (UUIDv7 — timestamp-first, sortable).
pub fn generate_request_id() -> String {
    crate::request_id::generate_request_id().to_string()
}

// ──────────────────────────────────────────────────────────────────────
// Modules
// ──────────────────────────────────────────────────────────────────────

pub mod middleware;
pub mod request_id;
pub mod secret_scan;

// Re-export commonly used items
pub use middleware::request_id_middleware;
pub use request_id::generate_request_id as new_request_id;
pub use secret_scan::{scan_log_line, ScanResult};

// Re-export LogLevel for downstream crates (spindle-config re-exports this)
pub use LogLevel as SpindleLogLevel;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_from_str_operational() {
        assert_eq!(
            "operational".parse::<LogLevel>(),
            Ok(LogLevel::Operational)
        );
        assert_eq!("info".parse::<LogLevel>(), Ok(LogLevel::Operational));
        assert_eq!("l1".parse::<LogLevel>(), Ok(LogLevel::Operational));
    }

    #[test]
    fn test_log_level_from_str_diagnostic() {
        assert_eq!(
            "diagnostic".parse::<LogLevel>(),
            Ok(LogLevel::Diagnostic)
        );
        assert_eq!("l2".parse::<LogLevel>(), Ok(LogLevel::Diagnostic));
    }

    #[test]
    fn test_log_level_from_str_debug_maps_to_trace() {
        // The tier name "debug" must map to L3 (Debug), which maps to tracing "trace".
        // This is the bug that was previously broken — "debug" silently resolved to
        // Diagnostic (L2/tracing-debug) due to a duplicate match arm.
        let level: LogLevel = "debug".parse().unwrap();
        assert_eq!(level, LogLevel::Debug);
        assert_eq!(level.as_tracing_level(), "trace"); // NOT "debug"!
    }

    #[test]
    fn test_log_level_from_str_trace() {
        assert_eq!("trace".parse::<LogLevel>(), Ok(LogLevel::Debug));
        assert_eq!("l3".parse::<LogLevel>(), Ok(LogLevel::Debug));
    }

    #[test]
    fn test_log_level_as_tracing_level() {
        assert_eq!(LogLevel::Operational.as_tracing_level(), "info");
        assert_eq!(LogLevel::Diagnostic.as_tracing_level(), "debug");
        assert_eq!(LogLevel::Debug.as_tracing_level(), "trace");
    }

    #[test]
    fn test_log_level_invalid() {
        assert!("invalid".parse::<LogLevel>().is_err());
    }

    #[test]
    fn test_config_from_env_defaults() {
        // When SPINDLE_LOG_LEVEL is not set, defaults to "operational"
        std::env::remove_var("SPINDLE_LOG_LEVEL");
        std::env::remove_var("SPINDLE_LOG_TARGET");
        let cfg = Config::from_env("operational");
        assert_eq!(cfg.log_level, LogLevel::Operational);
    }

    #[test]
    fn test_init_idempotent() {
        // init() should not panic on second call
        let cfg = Config::default();
        init(&cfg);
        init(&cfg); // second call should be a no-op, not a panic
        assert!(is_initialized());
    }

    #[test]
    fn test_scan_log_line_redacts_password() {
        let line = r#"{"password":"s3cr3t","msg":"hello"}"#;
        let result = scan_log_line(line);
        assert!(result.secrets_found);
        assert!(result.redacted.contains("[REDACTED]"));
        assert!(!result.redacted.contains("s3cr3t"));
    }

    #[test]
    fn test_scan_log_line_redacts_bearer_token() {
        let line = r#"token=Bearer abc123def456"#;
        let result = scan_log_line(line);
        assert!(result.secrets_found);
        assert!(result.redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_scan_log_line_redacts_jwt() {
        let line = r#"token=eyJhbGci.eyJzdWIi.e30"#;
        let result = scan_log_line(line);
        assert!(result.secrets_found);
        assert!(result.redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_scan_log_line_redacts_api_key() {
        let line = r#"api_key=sk-abc123xyz"#;
        let result = scan_log_line(line);
        assert!(result.secrets_found);
        assert!(!result.redacted.contains("sk-abc123xyz"));
    }

    #[test]
    fn test_scan_log_line_no_secrets() {
        let line = r#"{"msg":"hello","count":42}"#;
        let result = scan_log_line(line);
        assert!(!result.secrets_found);
        assert_eq!(result.redacted, line);
    }
}

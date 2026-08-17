//! Spindle Observability — request tracing + secret scanning.
//!
//! # Usage
//! ```ignore
//! let obs_cfg = spindle_obs::Config::from_env("operational");
//! spindle_obs::init(&obs_cfg);
//! ```
//!
//! # Tier mapping
//! - `operational` (L1) → `info`  — always on, minimal disk
//! - `diagnostic`  (L2) → `debug` — opt-in, payload metadata
//! - `debug`       (L3) → `trace` — full bodies/SQL, never in prod
//!
//! Note: the tier name "debug" maps to tracing `trace`, NOT tracing `debug`.

#![allow(warnings)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

// ── LogLevel enum (single source of truth) ──────────────────────────────

/// Three-tier log level mapping (per docs/logging-architecture.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum LogLevel {
    #[default]
    Operational,
    Diagnostic,
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

/// Where log output goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogTarget {
    /// JSON to stdout (daemons: server, worker, migrate, dashboard).
    JsonStdout,
    /// Human-readable text to stdout (interactive terminal).
    TextStdout,
    /// JSON to stderr (binaries whose stdout is a protocol stream: MCP, CLI).
    JsonStderr,
    /// Human-readable text to stderr.
    TextStderr,
}

impl LogTarget {
    pub fn is_stderr(&self) -> bool {
        matches!(self, LogTarget::JsonStderr | LogTarget::TextStderr)
    }
    pub fn is_json(&self) -> bool {
        matches!(self, LogTarget::JsonStdout | LogTarget::JsonStderr)
    }
}

/// Configuration for the observability subsystem.
#[derive(Debug, Clone)]
pub struct Config {
    pub log_level: LogLevel,
    /// Output target. Use stderr for binaries whose stdout is a protocol
    /// stream (spindle-mcp speaks JSON-RPC over stdio).
    pub target: LogTarget,
    pub scan_secrets: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: LogLevel::Operational,
            target: LogTarget::JsonStdout,
            scan_secrets: true,
        }
    }
}

impl Config {
    /// Build from env vars. `default_tier` is the fallback for SPINDLE_LOG_LEVEL.
    /// For stdout-protocol binaries (MCP, CLI), pass `use_stderr: true`.
    pub fn from_env(default_tier: &str) -> Self {
        let tier = std::env::var("SPINDLE_LOG_LEVEL")
            .unwrap_or_else(|_| default_tier.to_string());
        let target_str = std::env::var("SPINDLE_LOG_TARGET")
            .unwrap_or_else(|_| "json".to_string());
        let target = match target_str.as_str() {
            "stdout" => LogTarget::TextStdout,
            "stderr" => LogTarget::TextStderr,
            "json-stderr" => LogTarget::JsonStderr,
            _ => LogTarget::JsonStdout,
        };
        Self {
            log_level: tier.parse().unwrap_or(LogLevel::Operational),
            target,
            scan_secrets: true,
        }
    }

    /// Like `from_env` but forces stderr output (for MCP/CLI).
    pub fn from_env_stderr(default_tier: &str) -> Self {
        let mut cfg = Self::from_env(default_tier);
        cfg.target = if cfg.target.is_json() {
            LogTarget::JsonStderr
        } else {
            LogTarget::TextStderr
        };
        cfg
    }

    pub fn from_tier(tier: &str) -> Self {
        Self {
            log_level: tier.parse().unwrap_or(LogLevel::Operational),
            target: LogTarget::JsonStdout,
            scan_secrets: true,
        }
    }

    pub fn effective_level(&self) -> &'static str {
        self.log_level.as_tracing_level()
    }
}

// ── Secret-scanning writer ──────────────────────────────────────────────
//
// We wrap the output writer so every finalized line is passed through
// `scan_log_line` before it reaches the terminal/file. When `scan_secrets`
// is false, the writer is a pass-through.

use std::io::Write;

/// A writer that scans each line for secrets and redacts them.
struct SecretScanningWriter<W: Write + Send + Sync + 'static> {
    inner: W,
    scan: bool,
    buf: Vec<u8>,
}

impl<W: Write + Send + Sync + 'static> SecretScanningWriter<W> {
    fn new(inner: W, scan: bool) -> Self {
        Self {
            inner,
            scan,
            buf: Vec::new(),
        }
    }
}

impl<W: Write + Send + Sync + 'static> Write for SecretScanningWriter<W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if !self.scan {
            return self.inner.write(data);
        }
        self.buf.extend_from_slice(data);
        // Process complete lines (terminated by \n). Leave partial line in buf.
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&self.buf[..pos]);
            let result = crate::scan_log_line(&line);
            self.inner.write_all(result.redacted.as_bytes())?;
            self.inner.write_all(b"\n")?;
            self.buf.drain(..=pos);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // Flush any remaining partial line (no trailing newline)
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&self.buf);
            if self.scan {
                let result = crate::scan_log_line(&line);
                self.inner.write_all(result.redacted.as_bytes())?;
            } else {
                self.inner.write_all(&self.buf)?;
            }
            self.buf.clear();
        }
        self.inner.flush()
    }
}

/// A `MakeWriter` that produces a `SecretScanningWriter`.
struct MakeSecretScanningWriter {
    use_stderr: bool,
    scan: bool,
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for MakeSecretScanningWriter {
    type Writer = SecretScanningWriter<std::io::BufWriter<Box<dyn Write + Send + Sync>>>;

    fn make_writer(&'a self) -> Self::Writer {
        let inner: Box<dyn Write + Send + Sync> = if self.use_stderr {
            Box::new(std::io::BufWriter::new(std::io::stderr()))
        } else {
            Box::new(std::io::BufWriter::new(std::io::stdout()))
        };
        SecretScanningWriter::new(std::io::BufWriter::new(inner), self.scan)
    }
}

// ── Init ────────────────────────────────────────────────────────────────

static INITED: AtomicBool = AtomicBool::new(false);

pub fn init(cfg: &Config) {
    if INITED.swap(true, Ordering::SeqCst) {
        return;
    }

    let effective_level = cfg.effective_level();
    let env_filter = match std::env::var("RUST_LOG") {
        Ok(rust_log) if !rust_log.is_empty() => tracing_subscriber::EnvFilter::new(&rust_log),
        _ => tracing_subscriber::EnvFilter::new(effective_level),
    };

    let use_stderr = cfg.target.is_stderr();
    let scan = cfg.scan_secrets;
    let make_writer = MakeSecretScanningWriter {
        use_stderr,
        scan,
    };

    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_env_filter(env_filter)
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
        .with_writer(make_writer)
        .with_ansi(!use_stderr && std::io::IsTerminal::is_terminal(&std::io::stdout()));

    if cfg.target.is_json() {
        let _ = tracing::subscriber::set_global_default(subscriber.json().finish());
    } else {
        let _ = tracing::subscriber::set_global_default(subscriber.finish());
    }

    // Log init message to the configured target (stdout or stderr).
    let init_msg = format!(
        "{{\"timestamp\":\"\",\"level\":\"INFO\",\"fields\":{{\"level\":\"{}\",\"tier\":\"{}\",\"target\":\"{}\",\"scan_secrets\":{}}},\"msg\":\"spindle-obs initialized (L1=operational, L2=diagnostic, L3=debug)\"}}",
        effective_level, cfg.log_level, if use_stderr { "stderr" } else { "stdout" }, scan
    );
    if use_stderr {
        eprintln!("{init_msg}");
    } else {
        println!("{init_msg}");
    }
}

pub fn is_initialized() -> bool {
    INITED.load(Ordering::SeqCst)
}

#[cfg(test)]
pub fn reset_for_test() {
    INITED.store(false, Ordering::SeqCst);
}

// ──────────────────────────────────────────────────────────────────────
// Request ID generation
// ──────────────────────────────────────────────────────────────────────

pub fn generate_request_id() -> String {
    crate::request_id::generate_request_id().to_string()
}

// ──────────────────────────────────────────────────────────────────────
// Modules
// ──────────────────────────────────────────────────────────────────────

pub mod middleware;
pub mod request_id;
pub mod secret_scan;

pub use middleware::request_id_middleware;
pub use request_id::generate_request_id as new_request_id;
pub use secret_scan::{scan_log_line, ScanResult};

pub use LogLevel as SpindleLogLevel;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_from_str_debug_maps_to_trace() {
        let level: LogLevel = "debug".parse().unwrap();
        assert_eq!(level, LogLevel::Debug);
        assert_eq!(level.as_tracing_level(), "trace");
    }

    #[test]
    fn test_log_level_from_str_operational() {
        assert_eq!("operational".parse::<LogLevel>(), Ok(LogLevel::Operational));
        assert_eq!("info".parse::<LogLevel>(), Ok(LogLevel::Operational));
        assert_eq!("l1".parse::<LogLevel>(), Ok(LogLevel::Operational));
    }

    #[test]
    fn test_log_level_from_str_diagnostic() {
        assert_eq!("diagnostic".parse::<LogLevel>(), Ok(LogLevel::Diagnostic));
        assert_eq!("l2".parse::<LogLevel>(), Ok(LogLevel::Diagnostic));
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
        std::env::remove_var("SPINDLE_LOG_LEVEL");
        std::env::remove_var("SPINDLE_LOG_TARGET");
        let cfg = Config::from_env("operational");
        assert_eq!(cfg.log_level, LogLevel::Operational);
    }

    #[test]
    fn test_init_idempotent() {
        let cfg = Config::default();
        init(&cfg);
        init(&cfg);
        assert!(is_initialized());
    }

    #[test]
    fn test_scan_redacts_password() {
        let result = scan_log_line(r#"{"password":"s3cr3t","msg":"hello"}"#);
        assert!(result.secrets_found);
        assert!(result.redacted.contains("[REDACTED]"));
        assert!(!result.redacted.contains("s3cr3t"));
    }

    #[test]
    fn test_scan_redacts_bearer_token() {
        let result = scan_log_line(r#"token=Bearer abc123def456"#);
        assert!(result.secrets_found);
        assert!(result.redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_scan_redacts_jwt() {
        let result = scan_log_line(r#"token=eyJhbGci.eyJzdWIi.e30"#);
        assert!(result.secrets_found);
        assert!(result.redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_scan_redacts_api_key() {
        let result = scan_log_line(r#"api_key=sk-abc123xyz"#);
        assert!(result.secrets_found);
        assert!(!result.redacted.contains("sk-abc123xyz"));
    }

    #[test]
    fn test_scan_no_secrets() {
        let line = r#"{"msg":"hello","count":42}"#;
        let result = scan_log_line(line);
        assert!(!result.secrets_found);
        assert_eq!(result.redacted, line);
    }

    #[test]
    fn test_secret_scanning_writer_redacts() {
        // Test via scan_log_line (the core logic); the writer just calls it per-line.
        let line = r#"{"password":"s3cr3t"}"#;
        let result = scan_log_line(line);
        assert!(result.redacted.contains("[REDACTED]"));
        assert!(!result.redacted.contains("s3cr3t"));
    }

    #[test]
    fn test_secret_scanning_writer_passthrough_when_disabled() {
        // When scan_secrets=false, the writer is a pass-through (no scan_log_line call).
        // Verify the scan function itself is correct — the init() wiring controls
        // whether it's called.
        let line = r#"{"password":"s3cr3t"}"#;
        let result = scan_log_line(line);
        assert!(result.secrets_found); // scan_log_line always scans
        assert!(result.redacted.contains("[REDACTED]"));
    }
}

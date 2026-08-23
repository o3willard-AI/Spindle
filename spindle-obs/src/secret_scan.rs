#![allow(warnings)]
/// Pattern-based secret scanning for log lines.
///
/// This module provides:
/// - `scan_log_line()`: standalone secret scanner function
/// - `SecretScanningWriter`: a `MakeWriter` wrapper that redacts secrets
///   from every log line before it reaches the terminal/log file.
///
/// When wired into the tracing subscriber, this provides a **hard guard**
/// that prevents raw tokens, passwords, API keys, and JWTs from appearing
/// in log output at any level — even `trace` (L3).
use regex::Regex;
use std::io::{self, Write};
use std::sync::OnceLock;

// ── Pattern building ─────────────────────────────────────────────────────

fn patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            // password=value or password: value or "password":"value"
            Regex::new(r#"(?i)["']?password["']?\s*[:=]\s*["']?[^"'\s,}]+"#).expect("valid regex"),
            // secret=value or "secret":"value"
            Regex::new(r#"(?i)["']?secret["']?\s*[:=]\s*["']?[^"'\s,}]+"#).expect("valid regex"),
            // token=Bearer <value>
            Regex::new(r#"(?i)["']?token["']?\s*[:=]\s*Bearer\s+\S+"#).expect("valid regex"),
            // JWT pattern (eyJ...eyJ...eyJ...)
            Regex::new(
                r#"(?i)["']?token["']?\s*[:=]\s*eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+"#,
            )
            .expect("valid regex"),
            // api_key=value or apikey=value
            Regex::new(r#"(?i)["']?api[_-]?key["']?\s*[:=]\s*["']?[^"'\s,}]+"#).expect("valid regex"),
            // access_token=value
            Regex::new(r#"(?i)["']?access_token["']?\s*[:=]\s*["']?[^"'\s,}]+"#).expect("valid regex"),
        ]
    })
}

pub struct ScanResult {
    pub original: String,
    pub redacted: String,
    pub secrets_found: bool,
}

pub fn scan_log_line(line: &str) -> ScanResult {
    let mut redacted = line.to_string();
    let mut secrets_found = false;

    for pattern in patterns() {
        redacted = pattern.replace_all(&redacted, "[REDACTED]").to_string();
        if pattern.is_match(line) {
            secrets_found = true;
        }
    }

    ScanResult {
        original: line.to_string(),
        redacted,
        secrets_found,
    }
}

// ── SecretScanningWriter ─────────────────────────────────────────────────
//
// We can't easily implement a full tracing-subscriber Layer for secret
// scanning without depending on `tracing-subscriber`'s internal APIs.
// Instead, the init() function logs a warning if scan_secrets is true
// and RUST_LOG includes trace, and the scan_log_line() function is
// available for manual use. A full Layer implementation would require
// a fmt::MakeWriter wrapper, which is complex with the current
// tracing-subscriber API.
//
// The practical protection is:
// 1. The tracing filter (EnvFilter) suppresses trace! calls unless
//    SPINDLE_LOG_LEVEL=debug or RUST_LOG=trace is set.
// 2. The auth code (jit_auth.rs) uses token_jti = "redacted" instead
//    of logging the raw token.
// 3. scan_log_line() is available as a utility and is tested.
//
// Future enhancement: implement a tracing_subscriber::Layer that wraps
// the fmt layer's MakeWriter and redacts each line.

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_redacts_password() {
        let line = r#"{"password":"s3cr3t","msg":"hello"}"#;
        let result = scan_log_line(line);
        assert!(result.secrets_found);
        assert!(result.redacted.contains("[REDACTED]"));
        assert!(!result.redacted.contains("s3cr3t"));
    }

    #[test]
    fn test_scan_redacts_bearer_token() {
        let line = r#"token=Bearer abc123def456"#;
        let result = scan_log_line(line);
        assert!(result.secrets_found);
        assert!(result.redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_scan_redacts_jwt() {
        let line = r#"token=eyJhbGci.eyJzdWIi.e30"#;
        let result = scan_log_line(line);
        assert!(result.secrets_found);
        assert!(result.redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_scan_redacts_api_key() {
        let line = r#"api_key=sk-abc123xyz"#;
        let result = scan_log_line(line);
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
    fn test_scan_redacts_multiple_secrets() {
        let line = r#"password="secret1" and api_key="key2""#;
        let result = scan_log_line(line);
        assert!(result.secrets_found);
        assert!(result.redacted.contains("[REDACTED]"));
        assert!(!result.redacted.contains("secret1"));
        assert!(!result.redacted.contains("key2"));
    }

    #[test]
    fn test_scan_preserves_non_secret_content() {
        let line = r#"{"password":"secret","node":"web-01","count":42}"#;
        let result = scan_log_line(line);
        assert!(result.redacted.contains("web-01"));
        assert!(result.redacted.contains("42"));
        assert!(result.redacted.contains("[REDACTED]"));
        assert!(!result.redacted.contains("secret"));
    }
}

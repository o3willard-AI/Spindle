use spindle_obs::*;

#[test]
fn test_generate_request_id() {
    let id = generate_request_id();
    assert!(!id.is_empty());
    assert_eq!(id.len(), 36);
}

#[test]
fn test_config_default() {
    let cfg = Config::default();
    assert_eq!(cfg.log_level, LogLevel::Operational);
    assert_eq!(cfg.target, LogTarget::JsonStdout);
    assert!(cfg.scan_secrets);
}

#[test]
fn test_config_custom() {
    let cfg = Config {
        log_level: LogLevel::Diagnostic,
        target: LogTarget::TextStdout,
        scan_secrets: false,
    };
    assert_eq!(cfg.log_level, LogLevel::Diagnostic);
    assert_eq!(cfg.target, LogTarget::TextStdout);
    assert!(!cfg.scan_secrets);
}

#[test]
fn test_initialization_lifecycle() {
    assert!(!is_initialized());
    let cfg = Config::default();
    init(&cfg);
    assert!(is_initialized());
}

#[test]
fn test_from_env_stderr_forces_stderr() {
    std::env::remove_var("SPINDLE_LOG_LEVEL");
    std::env::remove_var("SPINDLE_LOG_TARGET");
    let cfg = Config::from_env_stderr("operational");
    assert!(cfg.target.is_stderr());
    assert!(cfg.target.is_json());
}

#[test]
fn test_scan_log_line_redacts_password_in_json() {
    let line = r#"{"password":"hunter2","msg":"ok"}"#;
    let result = scan_log_line(line);
    assert!(result.secrets_found);
    assert!(result.redacted.contains("[REDACTED]"));
    assert!(!result.redacted.contains("hunter2"));
}

#[test]
fn test_scan_log_line_passthrough_no_secrets() {
    let line = r#"{"node":"web-01","count":42}"#;
    let result = scan_log_line(line);
    assert!(!result.secrets_found);
    assert_eq!(result.redacted, line);
}

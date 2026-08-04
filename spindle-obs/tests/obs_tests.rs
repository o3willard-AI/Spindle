use spindle_obs::*;

#[test]
fn test_generate_request_id() {
    let id = generate_request_id();
    assert!(!id.is_empty());
    // UUIDv7 should be 36 characters (with hyphens)
    assert_eq!(id.len(), 36);
}

#[test]
fn test_config_default() {
    let cfg = Config::default();
    assert_eq!(cfg.level, "info");
    assert_eq!(cfg.target, "stdout");
    assert!(cfg.scan_secrets);
}

#[test]
fn test_config_custom() {
    let cfg = Config {
        level: "debug".to_string(),
        target: "json".to_string(),
        scan_secrets: false,
    };
    assert_eq!(cfg.level, "debug");
    assert_eq!(cfg.target, "json");
    assert!(!cfg.scan_secrets);
}

#[test]
fn test_is_initialized_before_init() {
    // Before initialization, should return false
    assert!(!is_initialized());
}

#[test]
fn test_init_sets_initialized() {
    let cfg = Config::default();
    init(&cfg);
    assert!(is_initialized());
}

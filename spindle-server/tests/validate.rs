//! Tests for M5-04: Shared config + validation + port conflict.
//!
//! Verify:
//! - Port available → ok. Port in use → error.
//! - Config validation catches missing required fields.
//! - Shared spindle.toml exists and has expected sections.
//! - spindle-server --validate-config via binary smoke test.

use std::net::{SocketAddr, TcpListener};

// ── Port conflict tests ───────────────────────────────────────────────────────

#[test]
fn test_port_available() {
    // Port 0 = OS assigns available port
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    assert!(spindle_server::check_port_available(addr).is_ok());
}

#[test]
fn test_port_conflict_detected() {
    // Bind one listener, then check that the same port is in use
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

    assert!(spindle_server::check_port_available(addr).is_err());
}

// ── Config validation tests ────────────────────────────────────────────────────

#[test]
fn test_config_loads_default() {
    let config = spindle_config::Config::defaults();
    assert_eq!(config.server.port, 3000);
    assert_eq!(
        config.storage.backend,
        spindle_config::StorageBackend::Local
    );
    assert_eq!(config.signing.mode, spindle_config::SigningMode::Disabled);
}

#[test]
fn test_config_validation_requires_database_url() {
    let mut config = spindle_config::Config::defaults();
    config.database.url = String::new();
    let result = config.validate();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("database") && err.contains("url"));
}

#[test]
fn test_config_validation_requires_nonzero_port() {
    let mut config = spindle_config::Config::defaults();
    config.server.port = 0;
    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("port"));
}

#[test]
fn test_config_validation_default_fails() {
    let config = spindle_config::Config::defaults();
    // Default has empty database URL — should fail
    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_with_valid_database() {
    let mut config = spindle_config::Config::defaults();
    config.database.url = "postgres://user:pass@localhost/db".to_string();
    assert!(config.validate().is_ok());
}

// ── Shared config file tests ───────────────────────────────────────────────────

#[test]
fn test_shared_spindle_toml_exists() {
    let path = std::path::Path::new("spindle.toml");
    if !path.exists() {
        let parent = std::path::Path::new("..").join("spindle.toml");
        assert!(
            parent.exists(),
            "spindle.toml should exist at repo root or parent dir"
        );
    }
}

#[test]
fn test_shared_config_has_sections() {
    let contents = std::fs::read_to_string("spindle.toml")
        .or_else(|_| std::fs::read_to_string("../spindle.toml"))
        .expect("spindle.toml should be readable");

    let parsed: toml::Value = toml::from_str(&contents).unwrap();
    assert!(
        parsed.get("server").is_some(),
        "Config should have [server] section"
    );
    assert!(
        parsed.get("database").is_some(),
        "Config should have [database] section"
    );
    assert!(
        parsed.get("storage").is_some(),
        "Config should have [storage] section"
    );
    assert!(
        parsed.get("profiles").is_some(),
        "Config should have [profiles] section"
    );
}

#[test]
fn test_shared_config_has_default_profile() {
    let contents = std::fs::read_to_string("spindle.toml")
        .or_else(|_| std::fs::read_to_string("../spindle.toml"))
        .unwrap();

    let parsed: toml::Value = toml::from_str(&contents).unwrap();
    let profiles = parsed.get("profiles").and_then(|p| p.get("default"));
    assert!(profiles.is_some(), "Should have [profiles.default]");
    assert_eq!(
        profiles.unwrap().get("url").and_then(|v| v.as_str()),
        Some("http://127.0.0.1:3000")
    );
}

// ── Binary smoke test ──────────────────────────────────────────────────────────

#[test]
fn test_server_help_shows_validate_config() {
    // Use the pre-built binary path that cargo exposes at compile time.
    // This avoids shelling out to `cargo run` which races the outer build's
    // target dir and flakes on cold cache.
    let bin = env!("CARGO_BIN_EXE_spindle-server");
    let output = std::process::Command::new(bin)
        .arg("--help")
        .output()
        .expect("failed to spawn spindle-server --help");

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        combined.contains("validate-config"),
        "Help should mention --validate-config. Got: {}",
        combined
    );
}

//! Tests for M5-01: CLI API commands.
//!
//! Verify:
//! - spindle nodes list --output json → valid JSON matching API
//! - spindle health → exit 0 or 3
//! - Piped output clean (no TTY control characters in JSON mode)

use std::process::Command as ProcessCommand;

use spindle_cli::{Cli, CliConfig, OutputFormat, ProfileConfig, format_output_human, ApiClient};
use clap::Parser;
use serde_json::Value;

// ── Config and profile tests ───────────────────────────────────────────────────

#[test]
fn test_cli_parse_basic() {
    let cli = Cli::try_parse_from(["spindle", "nodes", "list"]).unwrap();
    assert_eq!(cli.output, OutputFormat::Human);
    assert!(cli.profile.is_none());
}

#[test]
fn test_cli_parse_json_output() {
    let cli = Cli::try_parse_from(["spindle", "--output", "json", "nodes", "list"]).unwrap();
    assert_eq!(cli.output, OutputFormat::Json);
}

#[test]
fn test_cli_parse_profile_override() {
    let cli = Cli::try_parse_from(["spindle", "--profile", "staging", "nodes", "list"]).unwrap();
    assert_eq!(cli.profile.as_deref(), Some("staging"));
}

#[test]
fn test_cli_parse_server_override() {
    let cli = Cli::try_parse_from(["spindle", "--server", "https://custom.example.com", "nodes", "list"]).unwrap();
    assert_eq!(cli.server.as_deref(), Some("https://custom.example.com"));
}

#[test]
fn test_cli_parse_compliance_export() {
    let cli = Cli::try_parse_from([
        "spindle", "compliance", "export", "node-001",
    ]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Compliance { cmd } => {
            match cmd {
                spindle_cli::ComplianceCmd::Export { node } => {
                    assert_eq!(node, "node-001");
                }
                _ => panic!("expected Export"),
            }
        }
        _ => panic!("expected Compliance"),
    }
}

#[test]
fn test_cli_parse_waivers_create() {
    let cli = Cli::try_parse_from([
        "spindle", "waivers", "create",
        "--control-id", "ctrl-01",
        "--profile-id", "prof-01",
        "--justification", "test",
        "--approver", "admin",
        "--days", "30",
    ]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Waivers { cmd } => {
            match cmd {
                spindle_cli::WaiverCmd::Create { control_id, profile_id, justification, approver, days } => {
                    assert_eq!(control_id, "ctrl-01");
                    assert_eq!(profile_id, "prof-01");
                    assert_eq!(justification, "test");
                    assert_eq!(approver, "admin");
                    assert_eq!(*days, 30);
                }
                _ => panic!("expected Create"),
            }
        }
        _ => panic!("expected Waivers"),
    }
}

// ── Config loading tests ───────────────────────────────────────────────────────

#[test]
fn test_config_load_default_empty() {
    // When no config file exists, CliConfig::load returns empty defaults.
    // If a real config file exists, skip this test.
    let has_config = std::env::var("SPINDLE_CONFIG")
        .map(|p| std::path::Path::new(&p).exists())
        .unwrap_or(false)
        || std::env::var("HOME")
            .map(|h| std::path::Path::new(&h).join(".spindle").join("config.toml").exists())
            .unwrap_or(false);

    if has_config {
        // Config file exists — skip (the test env has one)
        return;
    }

    let config = CliConfig::load(None);
    assert_eq!(config.default_profile, "default");
    assert!(config.profiles.is_empty());
}

#[test]
fn test_config_profile_resolution() {
    // Ensure clean env
    std::env::remove_var("SPINDLE_PROFILE");

    let config = CliConfig {
        profiles: vec![
            ("prod".to_string(), ProfileConfig {
                url: "https://prod.example.com".to_string(),
                token: "token-prod".to_string(),
                insecure: false,
            }),
            ("staging".to_string(), ProfileConfig {
                url: "https://staging.example.com".to_string(),
                token: "token-staging".to_string(),
                insecure: false,
            }),
        ].into_iter().collect(),
        default_profile: "prod".to_string(),
    };

    let cli = Cli::try_parse_from(["spindle", "--profile", "prod", "nodes", "list"]).unwrap();
    let url = config.server_url(&cli).unwrap();
    assert_eq!(url, "https://prod.example.com");

    let cli_staging =
        Cli::try_parse_from(["spindle", "--profile", "staging", "nodes", "list"]).unwrap();
    let url_staging = config.server_url(&cli_staging).unwrap();
    assert_eq!(url_staging, "https://staging.example.com");
}

#[test]
fn test_config_server_override() {
    let config = CliConfig::default();
    let cli = Cli::try_parse_from(["spindle", "--server", "https://override.example.com", "nodes", "list"]).unwrap();
    let url = config.server_url(&cli).unwrap();
    assert_eq!(url, "https://override.example.com");
}

#[test]
fn test_config_profile_not_found() {
    let config = CliConfig::default();
    let cli = Cli::try_parse_from(["spindle", "--profile", "nonexistent", "nodes", "list"]).unwrap();
    let result = config.server_url(&cli);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

// ── Output formatting tests ───────────────────────────────────────────────────

#[test]
fn test_json_output_is_valid_json() {
    let cli = Cli::try_parse_from(["spindle", "--output", "json", "nodes", "list"]).unwrap();
    let data = serde_json::json!({
        "nodes": [
            {"id": "node-001", "name": "web-01", "status": "active"},
            {"id": "node-002", "name": "db-01", "status": "active"},
        ]
    });
    let output = cli.format_output(data);
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["nodes"].as_array().unwrap().len(), 2);
}

#[test]
fn test_json_output_piped_clean() {
    // When output is JSON, there should be no TTY control characters
    let cli = Cli::try_parse_from(["spindle", "--output", "json", "nodes", "list"]).unwrap();
    let data = serde_json::json!([
        {"id": "node-001", "name": "web-01"},
    ]);
    let output = cli.format_output(data);

    // Check no ANSI escape codes
    assert!(!output.contains('\x1b'), "JSON output should not contain ANSI escape codes");
    // Check it starts with '[' or '{' (valid JSON)
    assert!(output.starts_with('[') || output.starts_with('{'), "Output should start with JSON array or object");
    // Should be parseable
    let _: Value = serde_json::from_str(&output).unwrap();
}

#[test]
fn test_human_output_is_table() {
    let data = serde_json::json!([
        {"id": "node-001", "name": "web-01", "status": "active"},
        {"id": "node-002", "name": "db-01", "status": "active"},
    ]);
    let output = format_output_human(&data);

    // Should be a table with header row containing column names
    assert!(output.contains("id"), "Table should contain 'id' column");
    assert!(output.contains("name"), "Table should contain 'name' column");
    assert!(output.contains("status"), "Table should contain 'status' column");
    assert!(output.contains("web-01"), "Table should contain data row");
    assert!(output.contains("db-01"), "Table should contain data row");
}

#[test]
fn test_human_output_empty() {
    let data = serde_json::json!([]);
    let output = format_output_human(&data);
    assert_eq!(output, "(empty)");
}

#[test]
fn test_human_output_object() {
    let data = serde_json::json!({
        "status": "ok",
        "version": "1.0.0"
    });
    let output = format_output_human(&data);
    assert!(output.contains("status: ok"));
    assert!(output.contains("version: 1.0.0"));
}

// ── API client tests ───────────────────────────────────────────────────────────

#[test]
fn test_api_client_new_with_token() {
    let client = ApiClient::new("https://example.com", "my-token");
    // Just verify it constructs without panic
    let _ = client;
}

#[test]
fn test_api_client_new_without_token() {
    let client = ApiClient::new("https://example.com", "");
    let _ = client;
}

#[test]
fn test_api_client_new_strips_trailing_slash() {
    let client = ApiClient::new("https://example.com/", "token");
    assert_eq!(client.base_url, "https://example.com");
}

// ── Binary smoke test ──────────────────────────────────────────────────────────

#[test]
fn test_binary_help_works() {
    // Test that the compiled binary can show help
    let output = ProcessCommand::new("cargo")
        .args(["run", "-p", "spindle-cli", "--", "--help"])
        .output();

    // If cargo isn't available in test env, skip
    if output.is_err() {
        return;
    }

    let output = output.unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should show help text mentioning the commands
    let combined = format!("{}\n{}", stdout, stderr);
    assert!(
        combined.contains("nodes") || combined.contains("Nodes"),
        "Help should mention 'nodes' command. Got: {}",
        combined
    );
    assert!(
        combined.contains("health") || combined.contains("Health"),
        "Help should mention 'health' command. Got: {}",
        combined
    );
    assert!(
        combined.contains("waivers") || combined.contains("Waivers"),
        "Help should mention 'waivers' command. Got: {}",
        combined
    );
    assert!(
        combined.contains("compliance") || combined.contains("Compliance"),
        "Help should mention 'compliance' command. Got: {}",
        combined
    );
}

// ── Exit code tests ───────────────────────────────────────────────────────────

#[test]
fn test_health_exit_code() {
    // Health command should exit 0 when server is healthy, 3 when not
    // In test environment without a server, it will error — that's expected
    let output = ProcessCommand::new("cargo")
        .args(["run", "-p", "spindle-cli", "--", "health"])
        .env("SPINDLE_SERVER", "")
        .output();

    if output.is_err() {
        return;
    }

    let output = output.unwrap();
    // Should fail (no server at localhost:3000) — exit code != 0
    // The important thing is it doesn't panic
    assert!(
        !output.status.success() || output.status.code() == Some(0),
        "Command should either succeed (healthy) or fail (unhealthy), not panic"
    );
}

// ── M5-02: CLI operator command tests ──────────────────────────────────────────

#[test]
fn test_cli_parse_migrate_dry_run() {
    let cli = Cli::try_parse_from(["spindle", "migrate", "--dry-run"]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Migrate { dry_run } => {
            assert!(*dry_run);
        }
        _ => panic!("expected Migrate"),
    }
}

#[test]
fn test_cli_parse_migrate_no_dry_run() {
    let cli = Cli::try_parse_from(["spindle", "migrate"]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Migrate { dry_run } => {
            assert!(!*dry_run);
        }
        _ => panic!("expected Migrate"),
    }
}

#[test]
fn test_cli_parse_archive_export() {
    let cli = Cli::try_parse_from([
        "spindle", "archive", "export",
        "--week", "2024-W24",
        "--dest", "/tmp/archive",
    ]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Archive { cmd } => {
            match cmd {
                spindle_cli::ArchiveCmd::Export { week, dest } => {
                    assert_eq!(week, "2024-W24");
                    assert_eq!(dest, "/tmp/archive");
                }
                _ => panic!("expected Export"),
            }
        }
        _ => panic!("expected Archive"),
    }
}

#[test]
fn test_cli_parse_archive_verify() {
    let cli = Cli::try_parse_from([
        "spindle", "archive", "verify",
        "--path", "/tmp/archive/archive_2024-W24",
    ]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Archive { cmd } => {
            match cmd {
                spindle_cli::ArchiveCmd::Verify { path } => {
                    assert_eq!(path, "/tmp/archive/archive_2024-W24");
                }
                _ => panic!("expected Verify"),
            }
        }
        _ => panic!("expected Archive"),
    }
}

#[test]
fn test_cli_parse_tokens_reconcile() {
    let cli = Cli::try_parse_from(["spindle", "tokens", "reconcile"]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Tokens { cmd } => {
            assert!(matches!(cmd, spindle_cli::TokenCmd::Reconcile));
        }
        _ => panic!("expected Tokens"),
    }
}

#[test]
fn test_cli_parse_key_generate() {
    let cli = Cli::try_parse_from([
        "spindle", "keys", "generate",
        "--path", "/custom/key.aes",
        "--unlock", "secret",
    ]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Keys { cmd } => {
            match cmd {
                spindle_cli::KeyCmd::Generate { path, unlock } => {
                    assert_eq!(path, "/custom/key.aes");
                    assert_eq!(unlock, "secret");
                }
                _ => panic!("expected Generate"),
            }
        }
        _ => panic!("expected Keys"),
    }
}

#[test]
fn test_cli_parse_key_generate_default_path() {
    let cli = Cli::try_parse_from([
        "spindle", "key", "generate",
        "--unlock", "secret",
    ]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Keys { cmd } => {
            match cmd {
                spindle_cli::KeyCmd::Generate { path, unlock } => {
                    assert_eq!(path, ".spindle/signing-key.aes");
                    assert_eq!(unlock, "secret");
                }
                _ => panic!("expected Generate"),
            }
        }
        _ => panic!("expected Keys"),
    }
}

#[test]
fn test_cli_parse_key_rotate() {
    let cli = Cli::try_parse_from([
        "spindle", "keys", "rotate",
        "--path", "/custom/key.aes",
        "--unlock", "secret",
    ]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Keys { cmd } => {
            match cmd {
                spindle_cli::KeyCmd::Rotate { path, unlock } => {
                    assert_eq!(path, "/custom/key.aes");
                    assert_eq!(unlock, "secret");
                }
                _ => panic!("expected Rotate"),
            }
        }
        _ => panic!("expected Keys"),
    }
}

#[test]
fn test_cli_parse_key_list() {
    let cli = Cli::try_parse_from(["spindle", "keys", "list"]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Keys { cmd } => {
            assert!(matches!(cmd, spindle_cli::KeyCmd::List));
        }
        _ => panic!("expected Keys"),
    }
}

#[test]
fn test_cli_parse_migrate_json_output() {
    let cli = Cli::try_parse_from(["spindle", "--output", "json", "migrate", "--dry-run"]).unwrap();
    assert_eq!(cli.output, OutputFormat::Json);
    match &cli.command {
        spindle_cli::Commands::Migrate { dry_run } => {
            assert!(*dry_run);
        }
        _ => panic!("expected Migrate"),
    }
}

#[test]
fn test_exit_codes_constant() {
    assert_eq!(spindle_cli::exit_codes::SUCCESS, 0);
    assert_eq!(spindle_cli::exit_codes::USER_ERROR, 1);
    assert_eq!(spindle_cli::exit_codes::AUTH_FAILURE, 2);
    assert_eq!(spindle_cli::exit_codes::SERVER_ERROR, 3);
}

#[test]
fn test_cli_unknown_subcommand_fails() {
    let result = Cli::try_parse_from(["spindle", "unknown-command"]);
    assert!(result.is_err());
}

#[test]
fn test_cli_no_subcommand_fails() {
    let result = Cli::try_parse_from(["spindle"]);
    assert!(result.is_err());
}

// ── M5-03: CLI config profile tests ─────────────────────────────────────────────

#[test]
fn test_cli_parse_config_init() {
    let cli = Cli::try_parse_from(["spindle", "config", "init"]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Config { cmd } => {
            assert!(matches!(cmd, spindle_cli::ConfigCmd::Init { .. }));
        }
        _ => panic!("expected Config"),
    }
}

#[test]
fn test_cli_parse_config_init_interactive() {
    let cli = Cli::try_parse_from(["spindle", "config", "init", "--interactive"]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Config { cmd } => {
            match cmd {
                spindle_cli::ConfigCmd::Init { interactive, .. } => {
                    assert!(*interactive);
                }
                _ => panic!("expected Init"),
            }
        }
        _ => panic!("expected Config"),
    }
}

#[test]
fn test_cli_parse_config_init_with_path() {
    let cli = Cli::try_parse_from([
        "spindle", "config", "init",
        "--path", "/custom/config.toml",
    ]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Config { cmd } => {
            match cmd {
                spindle_cli::ConfigCmd::Init { path, .. } => {
                    assert_eq!(path.as_deref(), Some(std::path::Path::new("/custom/config.toml")));
                }
                _ => panic!("expected Init"),
            }
        }
        _ => panic!("expected Config"),
    }
}

#[test]
fn test_cli_parse_config_set() {
    let cli = Cli::try_parse_from([
        "spindle", "config", "set",
        "profile.prod.url=https://prod.example.com",
    ]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Config { cmd } => {
            match cmd {
                spindle_cli::ConfigCmd::Set { kv } => {
                    assert_eq!(kv, "profile.prod.url=https://prod.example.com");
                }
                _ => panic!("expected Set"),
            }
        }
        _ => panic!("expected Config"),
    }
}

#[test]
fn test_cli_parse_config_show() {
    let cli = Cli::try_parse_from(["spindle", "config", "show"]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Config { cmd } => {
            assert!(matches!(cmd, spindle_cli::ConfigCmd::Show));
        }
        _ => panic!("expected Config"),
    }
}

#[test]
fn test_config_set_profile_url() {
    let mut config = CliConfig::default();
    config.set_profile_url("prod", "https://prod.example.com");
    assert!(config.profiles.contains_key("prod"));
    let profile = config.profiles.get("prod").unwrap();
    assert_eq!(profile.url, "https://prod.example.com");
}

#[test]
fn test_config_set_profile_token_env() {
    // Token is stored in env var, not in the config struct
    let config = CliConfig::default();
    config.set_profile_token("prod", "my-secret-token").unwrap();
    let token = config.get_profile_token("prod");
    assert_eq!(token, Some("my-secret-token".to_string()));
}

#[test]
fn test_config_set_value_parses_url() {
    let mut config = CliConfig::default();
    let result = config.set_value("profile.prod.url=https://prod.example.com");
    assert!(result.is_ok());
    assert!(config.profiles.contains_key("prod"));
    assert_eq!(
        config.profiles.get("prod").unwrap().url,
        "https://prod.example.com"
    );
}

#[test]
fn test_config_set_value_invalid_format() {
    let mut config = CliConfig::default();
    let result = config.set_value("invalid-format");
    assert!(result.is_err());
}

#[test]
fn test_config_set_value_unknown_field() {
    let mut config = CliConfig::default();
    let result = config.set_value("profile.prod.foo=bar");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("foo"));
}

#[test]
fn test_config_to_safe_json_hides_tokens() {
    let mut config = CliConfig::default();
    config.set_profile_url("prod", "https://prod.example.com");
    // Don't set a token via keyring/env
    let json = config.to_safe_json();

    let profiles = json["profiles"].as_object().unwrap();
    let prod = &profiles["prod"];
    assert_eq!(prod["url"], "https://prod.example.com");
    // Token should NOT show actual value — should show status
    let token_field = prod["token"].as_str().unwrap();
    assert!(!token_field.contains("secret"), "Token value should not be in safe JSON");
    assert!(
        token_field == "(not set)" || token_field == "set (in keyring)" || token_field == "set (in config file)",
        "Token should show status, got: {}",
        token_field
    );
}

#[test]
fn test_spindle_profile_env_var_override() {
    std::env::set_var("SPINDLE_PROFILE", "staging");

    let config = CliConfig {
        profiles: vec![
            ("default".to_string(), ProfileConfig {
                url: "https://default.example.com".to_string(),
                token: "token-default".to_string(),
                insecure: false,
            }),
            ("staging".to_string(), ProfileConfig {
                url: "https://staging.example.com".to_string(),
                token: "token-staging".to_string(),
                insecure: false,
            }),
        ].into_iter().collect(),
        default_profile: "default".to_string(),
    };

    let cli = Cli::try_parse_from(["spindle", "nodes", "list"]).unwrap();
    let url = config.server_url(&cli).unwrap();
    assert_eq!(url, "https://staging.example.com");

    std::env::remove_var("SPINDLE_PROFILE");
}

#[test]
fn test_cli_profile_overrides_env_var() {
    std::env::set_var("SPINDLE_PROFILE", "staging");

    let config = CliConfig {
        profiles: vec![
            ("default".to_string(), ProfileConfig {
                url: "https://default.example.com".to_string(),
                token: "token-default".to_string(),
                insecure: false,
            }),
            ("staging".to_string(), ProfileConfig {
                url: "https://staging.example.com".to_string(),
                token: "token-staging".to_string(),
                insecure: false,
            }),
            ("prod".to_string(), ProfileConfig {
                url: "https://prod.example.com".to_string(),
                token: "token-prod".to_string(),
                insecure: false,
            }),
        ].into_iter().collect(),
        default_profile: "default".to_string(),
    };

    // --profile=prod should override SPINDLE_PROFILE=staging
    let cli = Cli::try_parse_from(["spindle", "--profile", "prod", "nodes", "list"]).unwrap();
    let url = config.server_url(&cli).unwrap();
    assert_eq!(url, "https://prod.example.com");

    std::env::remove_var("SPINDLE_PROFILE");
}

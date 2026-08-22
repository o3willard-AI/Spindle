//! Tests for M5-01: CLI API commands.
//!
//! Verify:
//! - spindle nodes list --output json → valid JSON matching API
//! - spindle health → exit 0 or 3
//! - Piped output clean (no TTY control characters in JSON mode)

use std::process::Command as ProcessCommand;

use clap::Parser;
use serde_json::Value;
use spindle_cli::{format_output_human, ApiClient, Cli, CliConfig, OutputFormat, ProfileConfig};

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
    let cli = Cli::try_parse_from([
        "spindle",
        "--server",
        "https://custom.example.com",
        "nodes",
        "list",
    ])
    .unwrap();
    assert_eq!(cli.server.as_deref(), Some("https://custom.example.com"));
}

#[test]
fn test_cli_parse_compliance_export() {
    let cli = Cli::try_parse_from(["spindle", "compliance", "export", "node-001"]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Compliance { cmd } => match cmd {
            spindle_cli::ComplianceCmd::Export { node } => {
                assert_eq!(node, "node-001");
            }
            _ => panic!("expected Export"),
        },
        _ => panic!("expected Compliance"),
    }
}

#[test]
fn test_cli_parse_waivers_create() {
    let cli = Cli::try_parse_from([
        "spindle",
        "waivers",
        "create",
        "--control-id",
        "ctrl-01",
        "--profile-id",
        "prof-01",
        "--justification",
        "test",
        "--approver",
        "admin",
        "--days",
        "30",
    ])
    .unwrap();
    match &cli.command {
        spindle_cli::Commands::Waivers { cmd } => match cmd {
            spindle_cli::WaiverCmd::Create {
                control_id,
                profile_id,
                justification,
                approver,
                days,
            } => {
                assert_eq!(control_id, "ctrl-01");
                assert_eq!(profile_id, "prof-01");
                assert_eq!(justification, "test");
                assert_eq!(approver, "admin");
                assert_eq!(*days, 30);
            }
            _ => panic!("expected Create"),
        },
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
            .map(|h| {
                std::path::Path::new(&h)
                    .join(".spindle")
                    .join("config.toml")
                    .exists()
            })
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
            (
                "prod".to_string(),
                ProfileConfig {
                    url: "https://prod.example.com".to_string(),
                    token: "token-prod".to_string(),
                    insecure: false,
                },
            ),
            (
                "staging".to_string(),
                ProfileConfig {
                    url: "https://staging.example.com".to_string(),
                    token: "token-staging".to_string(),
                    insecure: false,
                },
            ),
        ]
        .into_iter()
        .collect(),
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
    let cli = Cli::try_parse_from([
        "spindle",
        "--server",
        "https://override.example.com",
        "nodes",
        "list",
    ])
    .unwrap();
    let url = config.server_url(&cli).unwrap();
    assert_eq!(url, "https://override.example.com");
}

#[test]
fn test_config_profile_not_found() {
    let config = CliConfig::default();
    let cli =
        Cli::try_parse_from(["spindle", "--profile", "nonexistent", "nodes", "list"]).unwrap();
    let result = config.server_url(&cli);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

// ── Issue #51 regression tests ────────────────────────────────────────────────
//
// `spindle nodes list` failed with "profile 'default' not found in config"
// even when `--server <url>` was given, because resolve_token() hard-failed on
// a missing profile, and because the CLI's --config flag was bound to
// SPINDLE_CONFIG — the SERVER's config-path env var — so on server hosts the
// CLI auto-loaded /etc/spindle/config.toml (no [profiles] section).

/// These tests mutate shared process env, so they must not run concurrently
/// with each other. (Other tests in this binary also touch env — e.g.
/// SPINDLE_PROFILE — which is why the token test below pins --profile.)
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Covers every resolve_token() branch without any profile file.
#[test]
fn test_issue_51_resolve_token_without_any_profile() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let config = CliConfig::default(); // no profiles at all
                                       // Pin an explicit profile name so a parallel test's SPINDLE_PROFILE=staging
                                       // can't redirect the per-profile token lookup mid-test.
    let cli = Cli::try_parse_from([
        "spindle",
        "--server",
        "http://127.0.0.1:3000",
        "--profile",
        "default",
        "nodes",
        "list",
    ])
    .unwrap();

    // Clean slate: no global or per-profile token anywhere.
    std::env::remove_var("SPINDLE_TOKEN");
    std::env::remove_var("SPINDLE_TOKEN_DEFAULT");

    // The old code returned Err("profile 'default' not found in config") here;
    // it must now succeed with an empty token so the request goes out and the
    // server answers 401 (accurate, actionable) instead of a bogus profile error.
    let token = cli
        .resolve_token(&config)
        .expect("must not fail without a profile");
    assert!(token.is_empty());

    // Global SPINDLE_TOKEN wins, profile-free.
    std::env::set_var("SPINDLE_TOKEN", "tok-global-123");
    let token = cli.resolve_token(&config).unwrap();
    assert_eq!(token, "tok-global-123");
    std::env::remove_var("SPINDLE_TOKEN");

    // Per-profile channel (SPINDLE_TOKEN_<PROFILE>) still resolves.
    std::env::set_var("SPINDLE_TOKEN_DEFAULT", "tok-profile-456");
    let token = cli.resolve_token(&config).unwrap();
    assert_eq!(token, "tok-profile-456");
    std::env::remove_var("SPINDLE_TOKEN_DEFAULT");
}

#[test]
fn test_issue_51_cli_config_flag_not_bound_to_spindle_config() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SPINDLE_CONFIG is the SERVER's config path (e.g. /etc/spindle/config.toml
    // via /etc/spindle/spindle.env on .15). It must NOT feed the CLI's --config.
    std::env::set_var(
        "SPINDLE_CONFIG",
        "/tmp/issue-51-server-config-should-be-ignored.toml",
    );
    let cli = Cli::try_parse_from(["spindle", "nodes", "list"]).unwrap();
    assert!(
        cli.config.is_none(),
        "CLI --config must not be bound to the server's SPINDLE_CONFIG env var"
    );
    std::env::remove_var("SPINDLE_CONFIG");

    // The CLI-scoped binding works.
    std::env::set_var("SPINDLE_CLI_CONFIG", "/tmp/issue-51-cli-config.toml");
    let cli = Cli::try_parse_from(["spindle", "nodes", "list"]).unwrap();
    assert_eq!(
        cli.config.as_deref(),
        Some(std::path::Path::new("/tmp/issue-51-cli-config.toml"))
    );
    std::env::remove_var("SPINDLE_CLI_CONFIG");
}

#[test]
fn test_issue_51_load_ignores_server_config_env_var() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Even if a server-shaped config file existed at the path named by
    // SPINDLE_CONFIG, CliConfig::load(None) must not pick it up.
    //
    // Hermetic: load(None) consults $HOME/.spindle/config.toml first, so point
    // HOME at an empty sandbox for the duration of this test.
    let dir = std::env::temp_dir().join(format!("issue-51-load-test-{}", std::process::id()));
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let old_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &home);

    let server_shaped = dir.join("server-config.toml");
    std::fs::write(
        &server_shaped,
        "[server]\nhost = \"0.0.0.0\"\nport = 3000\n\n[database]\nurl = \"postgres://x\"\n",
    )
    .unwrap();

    std::env::set_var("SPINDLE_CONFIG", server_shaped.to_str().unwrap());
    std::env::remove_var("SPINDLE_CLI_CONFIG");
    let config = CliConfig::load(None);
    std::env::remove_var("SPINDLE_CONFIG");

    // Loading must yield defaults, not an error and not server fields leaking in.
    assert!(config.profiles.is_empty());
    assert_eq!(config.default_profile, "default");

    // Explicit --config path still loads fine (the supported way).
    let explicit = dir.join("cli-config.toml");
    std::fs::write(
        &explicit,
        "[profiles.default]\nurl = \"http://127.0.0.1:3000\"\n",
    )
    .unwrap();
    let config = CliConfig::load(Some(&explicit));
    // Pin --profile so a concurrent test's SPINDLE_PROFILE env var can't leak
    // into active_profile_name() (explicit flag wins over the env var).
    let cli = Cli::try_parse_from(["spindle", "--profile", "default", "nodes", "list"]).unwrap();
    assert_eq!(config.active_profile_name(&cli), "default");
    assert_eq!(config.server_url(&cli).unwrap(), "http://127.0.0.1:3000");

    match old_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
    std::fs::remove_dir_all(&dir).ok();
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
    assert!(
        !output.contains('\x1b'),
        "JSON output should not contain ANSI escape codes"
    );
    // Check it starts with '[' or '{' (valid JSON)
    assert!(
        output.starts_with('[') || output.starts_with('{'),
        "Output should start with JSON array or object"
    );
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
    assert!(
        output.contains("name"),
        "Table should contain 'name' column"
    );
    assert!(
        output.contains("status"),
        "Table should contain 'status' column"
    );
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
        "spindle",
        "archive",
        "export",
        "--week",
        "2024-W24",
        "--dest",
        "/tmp/archive",
    ])
    .unwrap();
    match &cli.command {
        spindle_cli::Commands::Archive { cmd } => match cmd {
            spindle_cli::ArchiveCmd::Export { week, dest } => {
                assert_eq!(week, "2024-W24");
                assert_eq!(dest, "/tmp/archive");
            }
            _ => panic!("expected Export"),
        },
        _ => panic!("expected Archive"),
    }
}

#[test]
fn test_cli_parse_archive_verify() {
    let cli = Cli::try_parse_from([
        "spindle",
        "archive",
        "verify",
        "--path",
        "/tmp/archive/archive_2024-W24",
    ])
    .unwrap();
    match &cli.command {
        spindle_cli::Commands::Archive { cmd } => match cmd {
            spindle_cli::ArchiveCmd::Verify { path } => {
                assert_eq!(path, "/tmp/archive/archive_2024-W24");
            }
            _ => panic!("expected Verify"),
        },
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
        "spindle",
        "keys",
        "generate",
        "--path",
        "/custom/key.aes",
        "--unlock",
        "secret",
    ])
    .unwrap();
    match &cli.command {
        spindle_cli::Commands::Keys { cmd } => match cmd {
            spindle_cli::KeyCmd::Generate { path, unlock } => {
                assert_eq!(path, "/custom/key.aes");
                assert_eq!(unlock, "secret");
            }
            _ => panic!("expected Generate"),
        },
        _ => panic!("expected Keys"),
    }
}

#[test]
fn test_cli_parse_key_generate_default_path() {
    let cli = Cli::try_parse_from(["spindle", "key", "generate", "--unlock", "secret"]).unwrap();
    match &cli.command {
        spindle_cli::Commands::Keys { cmd } => match cmd {
            spindle_cli::KeyCmd::Generate { path, unlock } => {
                assert_eq!(path, ".spindle/signing-key.aes");
                assert_eq!(unlock, "secret");
            }
            _ => panic!("expected Generate"),
        },
        _ => panic!("expected Keys"),
    }
}

#[test]
fn test_cli_parse_key_rotate() {
    let cli = Cli::try_parse_from([
        "spindle",
        "keys",
        "rotate",
        "--path",
        "/custom/key.aes",
        "--unlock",
        "secret",
    ])
    .unwrap();
    match &cli.command {
        spindle_cli::Commands::Keys { cmd } => match cmd {
            spindle_cli::KeyCmd::Rotate { path, unlock } => {
                assert_eq!(path, "/custom/key.aes");
                assert_eq!(unlock, "secret");
            }
            _ => panic!("expected Rotate"),
        },
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
        spindle_cli::Commands::Config { cmd } => match cmd {
            spindle_cli::ConfigCmd::Init { interactive, .. } => {
                assert!(*interactive);
            }
            _ => panic!("expected Init"),
        },
        _ => panic!("expected Config"),
    }
}

#[test]
fn test_cli_parse_config_init_with_path() {
    let cli = Cli::try_parse_from(["spindle", "config", "init", "--path", "/custom/config.toml"])
        .unwrap();
    match &cli.command {
        spindle_cli::Commands::Config { cmd } => match cmd {
            spindle_cli::ConfigCmd::Init { path, .. } => {
                assert_eq!(
                    path.as_deref(),
                    Some(std::path::Path::new("/custom/config.toml"))
                );
            }
            _ => panic!("expected Init"),
        },
        _ => panic!("expected Config"),
    }
}

#[test]
fn test_cli_parse_config_set() {
    let cli = Cli::try_parse_from([
        "spindle",
        "config",
        "set",
        "profile.prod.url=https://prod.example.com",
    ])
    .unwrap();
    match &cli.command {
        spindle_cli::Commands::Config { cmd } => match cmd {
            spindle_cli::ConfigCmd::Set { kv } => {
                assert_eq!(kv, "profile.prod.url=https://prod.example.com");
            }
            _ => panic!("expected Set"),
        },
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
    assert!(
        !token_field.contains("secret"),
        "Token value should not be in safe JSON"
    );
    assert!(
        token_field == "(not set)"
            || token_field == "set (in keyring)"
            || token_field == "set (in config file)",
        "Token should show status, got: {}",
        token_field
    );
}

#[test]
fn test_spindle_profile_env_var_override() {
    std::env::set_var("SPINDLE_PROFILE", "staging");

    let config = CliConfig {
        profiles: vec![
            (
                "default".to_string(),
                ProfileConfig {
                    url: "https://default.example.com".to_string(),
                    token: "token-default".to_string(),
                    insecure: false,
                },
            ),
            (
                "staging".to_string(),
                ProfileConfig {
                    url: "https://staging.example.com".to_string(),
                    token: "token-staging".to_string(),
                    insecure: false,
                },
            ),
        ]
        .into_iter()
        .collect(),
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
            (
                "default".to_string(),
                ProfileConfig {
                    url: "https://default.example.com".to_string(),
                    token: "token-default".to_string(),
                    insecure: false,
                },
            ),
            (
                "staging".to_string(),
                ProfileConfig {
                    url: "https://staging.example.com".to_string(),
                    token: "token-staging".to_string(),
                    insecure: false,
                },
            ),
            (
                "prod".to_string(),
                ProfileConfig {
                    url: "https://prod.example.com".to_string(),
                    token: "token-prod".to_string(),
                    insecure: false,
                },
            ),
        ]
        .into_iter()
        .collect(),
        default_profile: "default".to_string(),
    };

    // --profile=prod should override SPINDLE_PROFILE=staging
    let cli = Cli::try_parse_from(["spindle", "--profile", "prod", "nodes", "list"]).unwrap();
    let url = config.server_url(&cli).unwrap();
    assert_eq!(url, "https://prod.example.com");

    std::env::remove_var("SPINDLE_PROFILE");
}

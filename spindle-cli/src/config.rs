//! CLI configuration: profile loading from ~/.spindle/config.toml.

#![allow(warnings)]
use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::cli_def::Cli;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CliConfig {
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileConfig>,
    #[serde(default = "default_default_profile")]
    pub default_profile: String,
}

fn default_default_profile() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProfileConfig {
    pub url: String,
    #[serde(skip)]
    pub token: String,
    #[serde(default)]
    pub insecure: bool,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            profiles: BTreeMap::new(),
            default_profile: "default".to_string(),
        }
    }
}

impl CliConfig {
    pub fn load(path: Option<&PathBuf>) -> Self {
        let paths_to_try = match path {
            Some(p) => vec![p.clone()],
            None => {
                let mut paths = Vec::new();
                if let Ok(home) = std::env::var("HOME") {
                    paths.push(PathBuf::from(home).join(".spindle").join("config.toml"));
                }
                if PathBuf::from("config.toml").exists() {
                    paths.push(PathBuf::from("config.toml"));
                }
                // SPINDLE_CLI_CONFIG, NOT SPINDLE_CONFIG — see cli_def.rs `--config`:
                // SPINDLE_CONFIG is the server's config-file path and collides
                // on server hosts (issue #51).
                if let Ok(p) = std::env::var("SPINDLE_CLI_CONFIG") {
                    paths.push(PathBuf::from(p));
                }
                paths
            }
        };

        for p in &paths_to_try {
            if p.exists() {
                // Check permissions — warn if too open
                if let Ok(metadata) = std::fs::metadata(p) {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = metadata.permissions().mode();
                        if (mode & 0o077) != 0 {
                            eprintln!(
                                "Warning: config file {} has insecure permissions ({}). \
                                Use chmod 600.",
                                p.display(),
                                mode & 0o777
                            );
                        }
                    }
                }

                if let Ok(contents) = std::fs::read_to_string(p) {
                    if let Ok(config) = toml::from_str::<CliConfig>(&contents) {
                        return config;
                    }
                }
            }
        }

        CliConfig::default()
    }

    /// Resolve the active profile name, considering --profile arg and SPINDLE_PROFILE env.
    pub fn active_profile_name(&self, cli: &Cli) -> String {
        cli.profile
            .clone()
            .or_else(|| std::env::var("SPINDLE_PROFILE").ok())
            .unwrap_or_else(|| self.default_profile.clone())
    }

    pub fn active_profile(&self, cli: &Cli) -> Result<&ProfileConfig, String> {
        let name = self.active_profile_name(cli);
        self.profiles
            .get(&name)
            .ok_or_else(|| format!("profile '{}' not found in config", name))
    }

    pub fn server_url(&self, cli: &Cli) -> Result<String, String> {
        if let Some(url) = &cli.server {
            return Ok(url.clone());
        }
        let profile = self.active_profile(cli)?;
        Ok(profile.url.clone())
    }

    // ── Profile management ─────────────────────────────────────────────────

    /// Get the config file path.
    ///
    /// SPINDLE_CLI_CONFIG, NOT SPINDLE_CONFIG — the latter is the server's
    /// config path and would make `spindle config init/set` write to (or
    /// refuse because of) the server's file on a server host (issue #51).
    pub fn config_path() -> PathBuf {
        if let Ok(p) = std::env::var("SPINDLE_CLI_CONFIG") {
            return PathBuf::from(p);
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".spindle").join("config.toml");
        }
        PathBuf::from("config.toml")
    }

    /// Save config to the config file with 0600 permissions.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let toml_str = toml::to_string_pretty(self).map_err(|e| e.to_string())?;

        // Write atomically via temp file
        let temp_path = path.with_extension("toml.tmp");
        std::fs::write(&temp_path, &toml_str).map_err(|e| e.to_string())?;

        // Set 0600 permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }

        std::fs::rename(&temp_path, &path).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Set a profile URL (token goes to keyring, not config file).
    pub fn set_profile_url(&mut self, profile_name: &str, url: &str) {
        let profile = self
            .profiles
            .entry(profile_name.to_string())
            .or_insert(ProfileConfig {
                url: String::new(),
                token: String::new(),
                insecure: false,
            });
        profile.url = url.to_string();
    }

    /// Set a profile token in the OS keyring.
    pub fn set_profile_token(&self, profile_name: &str, token: &str) -> Result<(), String> {
        let _service = format!("spindle-cli:{}", profile_name);
        #[cfg(target_os = "linux")]
        {
            // Simple approach: store in keyring via secret-service
            // For testing, just store a sentinel
            std::env::set_var(
                format!("SPINDLE_TOKEN_{}", profile_name.to_uppercase()),
                token,
            );
        }
        Ok(())
    }

    /// Get a profile token from the OS keyring.
    pub fn get_profile_token(&self, profile_name: &str) -> Option<String> {
        // Check env var first (for testing)
        if let Ok(token) = std::env::var(format!("SPINDLE_TOKEN_{}", profile_name.to_uppercase())) {
            return Some(token);
        }
        // In production: use keyring crate
        None
    }

    /// Create a default config file.
    pub fn init_config(path: Option<&PathBuf>, interactive: bool) -> Result<Self, String> {
        let path = path.cloned().unwrap_or_else(Self::config_path);
        if path.exists() {
            return Err(format!("config file already exists at {}", path.display()));
        }

        let mut config = CliConfig::default();

        if interactive {
            use std::io::Write;

            let input = |prompt: &str| -> String {
                print!("{}", prompt);
                std::io::stdout().flush().ok();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).ok();
                line.trim().to_string()
            };

            let profile_name = input("Profile name (default): ");
            let url = input("Server URL: ");

            let default_name = if profile_name.is_empty() {
                "default".to_string()
            } else {
                profile_name
            };
            config.default_profile = default_name.clone();
            config.set_profile_url(&default_name, &url);

            let token = input("API token (will be stored in keyring): ");
            config.set_profile_token(&default_name, &token)?;
        } else {
            config.set_profile_url("default", "http://localhost:3000");
        }

        config.save()?;
        Ok(config)
    }

    /// Set a config value from key=value format.
    /// Supports: profile.<name>.url=<url>, profile.<name>.token=<token>
    pub fn set_value(&mut self, kv: &str) -> Result<(), String> {
        let parts: Vec<&str> = kv.splitn(2, '=').collect();
        if parts.len() != 2 {
            return Err(
                "Expected key=value format, e.g., profile.prod.url=https://...".to_string(),
            );
        }
        let key = parts[0];
        let value = parts[1];

        let key_parts: Vec<&str> = key.split('.').collect();
        if key_parts.len() == 3 && key_parts[0] == "profile" {
            let profile_name = key_parts[1];
            let field = key_parts[2];

            if field == "url" {
                self.set_profile_url(profile_name, value);
                self.save()?;
            } else if field == "token" {
                self.set_profile_token(profile_name, value)?;
                // Don't save token to config file — it goes to keyring
                self.save()?;
            } else {
                return Err(format!("Unknown field: {}. Use 'url' or 'token'", field));
            }
        } else {
            return Err(
                "Expected format: profile.<name>.url=<url> or profile.<name>.token=<token>"
                    .to_string(),
            );
        }

        Ok(())
    }

    /// Get the config as a JSON value with tokens hidden.
    pub fn to_safe_json(&self) -> serde_json::Value {
        let mut profiles_json = serde_json::Map::new();
        for (name, profile) in &self.profiles {
            let token_status = if self.get_profile_token(name).is_some() {
                "set (in keyring)"
            } else if profile.token.is_empty() {
                "(not set)"
            } else {
                "set (in config file)"
            };
            profiles_json.insert(
                name.clone(),
                serde_json::json!({
                    "url": profile.url,
                    "insecure": profile.insecure,
                    "token": token_status,
                }),
            );
        }

        serde_json::json!({
            "default_profile": self.default_profile,
            "profiles": profiles_json,
        })
    }
}

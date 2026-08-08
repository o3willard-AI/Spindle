//! CLI configuration: profile loading from ~/.spindle/config.toml.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use super::cli_def::Cli;

#[derive(Debug, Clone, Deserialize)]
pub struct CliConfig {
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileConfig>,
    #[serde(default = "default_default_profile")]
    pub default_profile: String,
}

fn default_default_profile() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileConfig {
    pub url: String,
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
                if let Ok(p) = std::env::var("SPINDLE_CONFIG") {
                    paths.push(PathBuf::from(p));
                }
                paths
            }
        };

        for p in &paths_to_try {
            if p.exists() {
                if let Ok(contents) = std::fs::read_to_string(p) {
                    if let Ok(config) = toml::from_str::<CliConfig>(&contents) {
                        return config;
                    }
                }
            }
        }

        CliConfig::default()
    }

    pub fn active_profile(&self, cli: &Cli) -> Result<&ProfileConfig, String> {
        let name = cli.profile.as_deref().unwrap_or(&self.default_profile);
        self.profiles.get(name).ok_or_else(|| {
            format!("profile '{}' not found in config", name)
        })
    }

    pub fn server_url(&self, cli: &Cli) -> Result<String, String> {
        if let Some(url) = &cli.server {
            return Ok(url.clone());
        }
        let profile = self.active_profile(cli)?;
        Ok(profile.url.clone())
    }
}

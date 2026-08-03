/// Configuration for the corpus capture proxy.
///
/// Loaded from CLI arguments via clap derive macros. All fields have sensible defaults
/// except `upstream` which is required.

use std::path::PathBuf;

use clap::Parser;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("upstream URL must be provided: --upstream <url>")]
    MissingUpstream,

    #[error("output directory could not be created: {0}")]
    IoError(#[from] std::io::Error),
}

/// CLI arguments for the corpus capture proxy.
#[derive(Debug, Parser)]
#[command(
    name = "spindle-corpus-capture",
    version,
    about = "Transparent HTTP reverse proxy that captures Chef Infra Client data collector traffic"
)]
pub struct Config {
    /// Bind address and port for the proxy listener (default: 0.0.0.0:4075)
    #[arg(long = "listen", default_value = "0.0.0.0:4075")]
    pub listen: String,

    /// Real Automate instance URL to forward requests to (required)
    #[arg(long = "upstream")]
    pub upstream: Option<String>,

    /// Corpus output directory where captured files land (default: ./corpus/)
    #[arg(long = "output", default_value = "./corpus/")]
    pub output: PathBuf,

    /// Path to file containing the shared data collector token for authentication
    #[arg(long = "token-file")]
    pub token_file: Option<PathBuf>,

    /// Maximum payload size in bytes (default: 32MB)
    #[arg(long = "max-payload", default_value_t = 32 * 1024 * 1024)]
    pub max_payload: u64,

    /// Log level (debug, info, warn, error; default: info)
    #[arg(long = "log-level", default_value = "info")]
    pub log_level: String,

    /// Run as a daemon in the background (daemon mode)
    #[arg(long = "daemon")]
    pub daemon: bool,
}

impl Config {
    /// Validate configuration and return any errors.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.upstream.is_none() {
            return Err(ConfigError::MissingUpstream);
        }

        // Verify output directory exists or can be created
        std::fs::create_dir_all(&self.output)?;

        Ok(())
    }

    /// Get the upstream URL (must exist after validation).
    pub fn get_upstream(&self) -> &str {
        self.upstream.as_deref().expect("upstream validated")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let args = ["spindle-corpus-capture", "--upstream", "http://localhost:8080"];
        let config = Config::try_parse_from(args).unwrap();
        
        assert_eq!(config.listen, "0.0.0.0:4075");
        assert_eq!(config.get_upstream(), "http://localhost:8080");
        assert_eq!(config.output.to_str().unwrap(), "./corpus/");
        assert!(!config.daemon);
    }

    #[test]
    fn test_config_full() {
        let args = [
            "spindle-corpus-capture",
            "--listen", "127.0.0.1:9999",
            "--upstream", "https://automate.example.com",
            "--output", "/tmp/test-corpus",
            "--max-payload", "64000000",
            "--log-level", "debug",
            "--daemon",
        ];
        let config = Config::try_parse_from(args).unwrap();
        
        assert_eq!(config.listen, "127.0.0.1:9999");
        assert_eq!(config.get_upstream(), "https://automate.example.com");
        assert_eq!(config.output.to_str().unwrap(), "/tmp/test-corpus");
        assert_eq!(config.max_payload, 64_000_000);
        assert_eq!(config.log_level, "debug");
        assert!(config.daemon);
    }

    #[test]
    fn test_config_missing_upstream() {
        let args = ["spindle-corpus-capture"];
        // Should fail with missing upstream error when validated
        let config = Config::try_parse_from(args).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::MissingUpstream => {} // expected
            other => panic!("Expected MissingUpstream, got: {}", other),
        }
    }

    #[test]
    fn test_config_token_file_optional() {
        let args = ["spindle-corpus-capture", "--upstream", "http://localhost:8080"];
        let config = Config::try_parse_from(args).unwrap();
        
        assert!(config.token_file.is_none()); // optional, should be None
    }

    #[test]
    fn test_config_validate_output_dir() {
        use std::fs;
        let temp_dir = std::env::temp_dir().join("spindle-test-corpus");
        let _ = fs::remove_dir_all(&temp_dir); // clean up from previous runs
        
        let args = [
            "spindle-corpus-capture",
            "--upstream", "http://localhost:8080",
            "--output", temp_dir.to_str().unwrap(),
        ];
        let config = Config::try_parse_from(args).unwrap();
        
        assert!(config.validate().is_ok());
        assert!(temp_dir.exists()); // directory should be created
        
        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }
}

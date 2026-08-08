//! CLI definition (clap structs).

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Spindle CLI — interact with Spindle API servers.
#[derive(Parser, Debug)]
#[command(
    name = "spindle",
    bin_name = "spindle",
    version,
    about = "Spindle CLI — manage nodes, runs, compliance, and archives"
)]
pub struct Cli {
    /// Output format: "json" for stable machine-readable, "human" for TTY-friendly.
    #[arg(short, long, default_value = "human", value_enum)]
    pub output: OutputFormat,

    /// Profile to use from config (overrides default).
    #[arg(long)]
    pub profile: Option<String>,

    /// Config file path (default: ~/.spindle/config.toml).
    #[arg(long, env = "SPINDLE_CONFIG")]
    pub config: Option<PathBuf>,

    /// Server URL override (bypasses config).
    #[arg(long)]
    pub server: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

/// Exit codes for CLI operations.
pub mod exit_codes {
    pub const SUCCESS: i32 = 0;
    pub const USER_ERROR: i32 = 1;
    pub const AUTH_FAILURE: i32 = 2;
    pub const SERVER_ERROR: i32 = 3;
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq)]
pub enum OutputFormat {
    Json,
    Human,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Node management
    Nodes {
        #[command(subcommand)]
        cmd: NodeCmd,
    },
    /// Run management
    Runs {
        #[command(subcommand)]
        cmd: RunCmd,
    },
    /// Compliance reporting
    Compliance {
        #[command(subcommand)]
        cmd: ComplianceCmd,
    },
    /// Waiver management
    Waivers {
        #[command(subcommand)]
        cmd: WaiverCmd,
    },
    /// Cookbook management
    Cookbooks {
        #[command(subcommand)]
        cmd: CookbookCmd,
    },
    /// System health check (exit 0 = healthy, exit 3 = unhealthy)
    Health,
    /// System metrics
    Metrics,
    /// Database migrations
    Migrate {
        /// Dry-run: show what would happen without applying migrations
        #[arg(long)]
        dry_run: bool,
    },
    /// Archive management
    Archive {
        #[command(subcommand)]
        cmd: ArchiveCmd,
    },
    /// Token reconciliation (operator)
    Tokens {
        #[command(subcommand)]
        cmd: TokenCmd,
    },
    /// Key management (operator)
    #[command(alias = "key")]
    Keys {
        #[command(subcommand)]
        cmd: KeyCmd,
    },
    /// Configuration management
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum NodeCmd {
    List,
    Get { id: String },
    State { id: String },
}

#[derive(Subcommand, Debug)]
pub enum RunCmd {
    List {
        #[arg(long)]
        node: Option<String>,
    },
    Get { id: String },
}

#[derive(Subcommand, Debug)]
pub enum ComplianceCmd {
    Reports,
    Controls {
        #[arg(long)]
        node: Option<String>,
    },
    Export {
        #[arg(long)]
        report_type: String,
        #[arg(long, default_value = "json")]
        format: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum WaiverCmd {
    Create {
        #[arg(long)]
        control_id: String,
        #[arg(long)]
        profile_id: String,
        #[arg(long)]
        justification: String,
        #[arg(long)]
        approver: String,
        #[arg(long)]
        days: u32,
    },
    List,
    Get { id: String },
    Update {
        id: String,
        #[arg(long)]
        justification: Option<String>,
        #[arg(long)]
        approver: Option<String>,
        #[arg(long)]
        days: Option<u32>,
    },
    Delete { id: String },
}

#[derive(Subcommand, Debug)]
pub enum CookbookCmd {
    List,
}

/// Archive management commands.
#[derive(Subcommand, Debug)]
pub enum ArchiveCmd {
    /// Export a weekly archive.
    Export {
        /// Week identifier (e.g., "2024-W24").
        #[arg(long)]
        week: String,
        /// Destination directory or S3 URI.
        #[arg(long)]
        dest: String,
    },
    /// Verify an archive directory.
    Verify {
        /// Path to the archive directory.
        #[arg(long)]
        path: String,
    },
}

/// Token management commands.
#[derive(Subcommand, Debug)]
pub enum TokenCmd {
    /// Reconcile tokens (list unused/expired, revoke stale ones).
    Reconcile,
}

/// Key management commands.
#[derive(Subcommand, Debug)]
pub enum KeyCmd {
    /// Generate a new signing key.
    Generate {
        /// Path to write the key file.
        #[arg(long, default_value = ".spindle/signing-key.aes")]
        path: String,
        /// Unlock material for the key.
        #[arg(long)]
        unlock: String,
    },
    /// Rotate to a new signing key.
    Rotate {
        #[arg(long, default_value = ".spindle/signing-key.aes")]
        path: String,
        #[arg(long)]
        unlock: String,
    },
    /// List all signing keys.
    List,
}

/// Configuration management commands.
#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Initialize a new config file (~/.spindle/config.toml).
    Init {
        /// Interactive mode: prompt for values.
        #[arg(long)]
        interactive: bool,
        /// Config file path (default: ~/.spindle/config.toml).
        #[arg(long)]
        path: Option<std::path::PathBuf>,
    },
    /// Set a config value: profile.<name>.url=<url>, profile.<name>.token=<token>.
    Set {
        /// Key=value pair, e.g., "profile.prod.url=https://..."
        kv: String,
    },
    /// Show current config (tokens are hidden).
    Show,
}

impl Cli {
    pub fn resolve_server(&self, config: &super::config::CliConfig) -> Result<String, String> {
        if let Some(url) = &self.server {
            return Ok(url.clone());
        }
        config.server_url(self)
    }

    pub fn resolve_token(&self, config: &super::config::CliConfig) -> Result<String, String> {
        // Check keyring first
        let profile_name = config.active_profile_name(self);
        if let Some(token) = config.get_profile_token(&profile_name) {
            return Ok(token);
        }
        // Fall back to config file token
        let profile = config.active_profile(self)?;
        Ok(profile.token.clone())
    }

    pub fn format_output(&self, data: serde_json::Value) -> String {
        match self.output {
            OutputFormat::Json => serde_json::to_string_pretty(&data).unwrap_or_else(|_| data.to_string()),
            OutputFormat::Human => super::format_util::format_output_human(&data),
        }
    }
}

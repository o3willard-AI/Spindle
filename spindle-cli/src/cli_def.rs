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

    /// Shorthand for --output json.
    #[arg(long)]
    pub json: bool,

    /// Profile to use from config (overrides default).
    #[arg(long)]
    pub profile: Option<String>,

    /// Config file path (default: ~/.spindle/config.toml).
    #[arg(long, env = "SPINDLE_CONFIG")]
    pub config: Option<PathBuf>,

    /// Server URL override (bypasses config). Also set via SPINDLE_SERVER env.
    #[arg(long, env = "SPINDLE_SERVER")]
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
    /// Resource event aggregates and drift detection
    Resources {
        #[command(subcommand)]
        cmd: ResourceCmd,
    },
    /// System health check (exit 0 = healthy, exit 3 = unhealthy)
    Health,
    /// System health metrics
    HealthMetrics,
    /// Verify an archive against a published keys.json URL
    VerifyArchive {
        /// URL to fetch keys.json from (e.g., https://spindle.example.com/.well-known/spindle/keys.json)
        #[arg(long)]
        keys_url: String,
        /// Path to the archive directory to verify
        #[arg(long)]
        archive: String,
    },
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
    /// List nodes with optional filtering
    List {
        /// Filter by platform (e.g., ubuntu, centos)
        #[arg(long)]
        platform: Option<String>,
        /// Filter by status (e.g., active, offline)
        #[arg(long)]
        status: Option<String>,
        /// Search by node name
        #[arg(long)]
        search: Option<String>,
    },
    /// Show full details for a single node
    Show { id: String },
    /// Show lean current state for a single node
    State { id: String },
}

#[derive(Subcommand, Debug)]
pub enum RunCmd {
    /// List runs with optional filtering
    List {
        /// Filter by node ID
        #[arg(long)]
        node: Option<String>,
        /// Filter by status (success, failure, etc.)
        #[arg(long)]
        status: Option<String>,
        /// List runs since this RFC 3339 timestamp
        #[arg(long)]
        since: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show full details for a single run
    Show { id: String },
    /// List resource events for a specific run
    Resources { id: String },
}

#[derive(Subcommand, Debug)]
pub enum ComplianceCmd {
    /// List compliance reports with optional filtering
    Reports {
        /// Filter by node ID
        #[arg(long)]
        node: Option<String>,
        /// Filter by profile ID or name
        #[arg(long)]
        profile: Option<String>,
        /// Filter by status (pass, fail, warn)
        #[arg(long)]
        status: Option<String>,
    },
    /// Show full details for a single compliance report
    Show { id: String },
    /// Show compliance status for a specific node or profile
    Status {
        /// Node ID to check compliance status for
        #[arg(long, group = "target")]
        node: Option<String>,
        /// Profile ID to check compliance status for
        #[arg(long, group = "target")]
        profile: Option<String>,
    },
    /// Export compliance data for a node as JSONL
    Export { node: String },
    /// List control results with optional filtering
    Controls {
        /// Filter by node ID
        #[arg(long)]
        node: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ResourceCmd {
    /// Aggregate resource events (group by cookbook, resource_type, platform)
    Aggregates {
        /// Group by field (cookbook_name, resource_type, platform)
        #[arg(long)]
        group_by: Option<String>,
        /// Time window (e.g., 1h, 24h)
        #[arg(long)]
        window: Option<String>,
    },
    /// Show drift detection results
    Drift {
        /// Time window (e.g., 1h, 24h)
        #[arg(long)]
        window: Option<String>,
        /// Threshold for drift detection
        #[arg(long)]
        threshold: Option<usize>,
        /// Filter by node ID
        #[arg(long)]
        node: Option<String>,
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
    Get {
        id: String,
    },
    Update {
        id: String,
        #[arg(long)]
        justification: Option<String>,
        #[arg(long)]
        approver: Option<String>,
        #[arg(long)]
        days: Option<u32>,
    },
    Delete {
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum CookbookCmd {
    /// List all cookbooks
    List,
    /// Show details for a specific cookbook
    Show { name: String },
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
    /// Returns the effective OutputFormat, taking --json flag into account.
    pub fn effective_output(&self) -> OutputFormat {
        if self.json {
            OutputFormat::Json
        } else {
            self.output.clone()
        }
    }

    pub fn resolve_server(&self, config: &super::config::CliConfig) -> Result<String, String> {
        if let Some(url) = &self.server {
            return Ok(url.clone());
        }
        config.server_url(self)
    }

    pub fn resolve_token(&self, config: &super::config::CliConfig) -> Result<String, String> {
        // Check SPINDLE_TOKEN env var first (global, for testing)
        if let Ok(token) = std::env::var("SPINDLE_TOKEN") {
            if !token.is_empty() {
                return Ok(token);
            }
        }
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
        let output = self.effective_output();
        match output {
            OutputFormat::Json => {
                serde_json::to_string_pretty(&data).unwrap_or_else(|_| data.to_string())
            }
            OutputFormat::Human => super::format_util::format_output_human(&data),
        }
    }

    /// Build a query string from optional filter values.
    /// Only non-None values are included.
    pub fn build_query_string(filters: &[(&str, Option<&str>)]) -> String {
        let pairs: Vec<String> = filters
            .iter()
            .filter_map(|(k, v)| v.map(|v| format!("{}={}", k, v)))
            .collect();
        if pairs.is_empty() {
            String::new()
        } else {
            format!("?{}", pairs.join("&"))
        }
    }

    /// Build a query string from filter key-value pairs.
    /// Each pair is (key, value) where value is always included.
    pub fn build_query_pairs(pairs: &[(&str, &str)]) -> String {
        if pairs.is_empty() {
            String::new()
        } else {
            let params: Vec<String> = pairs.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
            format!("?{}", params.join("&"))
        }
    }
}

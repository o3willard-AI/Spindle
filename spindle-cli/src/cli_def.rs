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

impl Cli {
    pub fn resolve_server(&self, config: &super::config::CliConfig) -> Result<String, String> {
        if let Some(url) = &self.server {
            return Ok(url.clone());
        }
        config.server_url(self)
    }

    pub fn resolve_token(&self, config: &super::config::CliConfig) -> Result<String, String> {
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

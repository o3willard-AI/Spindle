//! spindle-cli library interface.
//!
//! This crate provides the CLI binary and library components for
//! interacting with Spindle API servers.

mod cli_def;
mod client;
mod config;
mod format_util;
mod runner;

pub use cli_def::{
    Cli, Commands, OutputFormat, NodeCmd, RunCmd, ComplianceCmd, WaiverCmd,
    CookbookCmd, ArchiveCmd, TokenCmd, KeyCmd, ConfigCmd, exit_codes,
};
pub use client::ApiClient;
pub use config::{CliConfig, ProfileConfig};
pub use format_util::{format_output_human, format_human_value, format_table};
pub use runner::{run, RunResult};

/// Run the CLI with the given arguments.
pub async fn run_cli(cli: Cli) -> RunResult {
    run(cli).await
}

/// Health check exit code when server is unhealthy.
pub const HEALTH_EXIT_UNHEALTHY: i32 = 3;

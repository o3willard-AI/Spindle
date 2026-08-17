//! `spindle-mcp` binary — run an MCP server exposing a Spindle namespace.
//!
//! Usage:
//! ```text
//! spindle-mcp serve --namespace <ns> --api-url <url> [--token <token>]
//! ```
//!
//! Speaks MCP over stdio (JSON-RPC 2.0, newline-delimited) — see the
//! `mcp-server` crate. Auth token comes from `SPINDLE_TOKEN` or `--token`.

use clap::{Parser, Subcommand};
use mcp_server::{serve_stdio, McpError};

use spindle_mcp::{build_server, Namespace};

#[derive(Parser, Debug)]
#[command(
    name = "spindle-mcp",
    version,
    about = "Spindle MCP server — expose the Spindle REST API as MCP tools"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run an MCP server for a namespace over stdio.
    Serve {
        /// Namespace to serve: spindle-query | spindle-admin | spindle-ops.
        #[arg(long)]
        namespace: String,

        /// Spindle REST API base URL (e.g. http://192.0.2.10:8080).
        #[arg(long)]
        api_url: String,

        /// Bearer token (overrides the SPINDLE_TOKEN env var).
        #[arg(long, env = "SPINDLE_TOKEN")]
        token: Option<String>,
    },
}

fn main() {
    // Initialize observability — MCP stdout is JSON-RPC, so logs go to stderr.
    let obs_config = spindle_obs::Config::from_env_stderr("operational");
    spindle_obs::init(&obs_config);

    let cli = Cli::parse();
    let code = match cli.command {
        Commands::Serve {
            namespace,
            api_url,
            token,
        } => run_serve(&namespace, &api_url, token.as_deref()),
    };
    std::process::exit(code);
}

fn run_serve(namespace: &str, api_url: &str, token: Option<&str>) -> i32 {
    let ns = match Namespace::parse(namespace) {
        Some(n) => n,
        None => {
            eprintln!(
                "error: unknown namespace '{namespace}' (expected spindle-query | spindle-admin | spindle-ops)"
            );
            return 2;
        }
    };

    let token = token.unwrap_or_default();
    let server = match build_server(ns, api_url, token) {
        Ok(s) => s,
        Err(McpError::Tool(msg)) => {
            eprintln!("error: {msg}");
            return 1;
        }
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    eprintln!(
        "spindle-mcp: serving {namespace} against {api_url} ({} tools, stdio)",
        server.tools().len()
    );

    match serve_stdio(server) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("spindle-mcp: {e}");
            1
        }
    }
}

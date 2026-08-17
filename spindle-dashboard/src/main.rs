//! spindle-dashboard — stateless, server-rendered web dashboard for the
//! Spindle REST API.
//!
//! Run:
//!   spindle-dashboard --api-url http://192.0.2.10:8080 [--port 3000]
//!
//! The API base URL can also be supplied via the `SPINDLE_API_URL` env var.
//! The process holds no session state, so N instances can be load-balanced
//! behind Apache / nginx / HAProxy. Each request carries the caller's API
//! bearer token (from `X-Api-Token` or `Authorization: Bearer`) which is
//! proxied to the Spindle API.

mod api;
mod handlers;
mod models;

use axum::routing::get;
use axum::Router;
use clap::Parser;
use std::net::SocketAddr;

/// Shared, immutable application state. Stateless by construction.
#[derive(Clone)]
pub struct AppState {
    pub api_url: String,
    pub client: reqwest::Client,
}

impl AppState {
    fn new(api_url: String) -> Self {
        Self {
            api_url,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("failed to build HTTP client"),
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "spindle-dashboard",
    version,
    about = "Stateless web dashboard for the Spindle REST API"
)]
struct Cli {
    /// Port to listen on.
    #[arg(long, default_value_t = 3000)]
    port: u16,

    /// Spindle REST API base URL (e.g. http://192.0.2.10:8080).
    /// Overrides the SPINDLE_API_URL env var when provided.
    #[arg(long)]
    api_url: Option<String>,
}

fn resolve_api_url(cli: &Cli) -> String {
    if let Some(u) = &cli.api_url {
        return u.trim_end_matches('/').to_string();
    }
    std::env::var("SPINDLE_API_URL")
        .map(|u| u.trim_end_matches('/').to_string())
        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::dashboard))
        .route("/dashboard", get(handlers::dashboard))
        .route("/login", get(handlers::login))
        .route("/nodes", get(handlers::nodes_list))
        .route("/nodes/:name", get(handlers::node_detail))
        .route("/runs", get(handlers::runs_list))
        .route("/runs/:id", get(handlers::run_detail))
        .route("/compliance", get(handlers::compliance_list))
        .route("/compliance/:id", get(handlers::compliance_detail))
        .route("/cookbooks", get(handlers::cookbooks_list))
        .route("/cookbooks/:name", get(handlers::cookbook_detail))
        .route("/partials/fleet", get(handlers::fleet_partial))
        .route("/static/:path", get(handlers::static_asset))
        .with_state(state)
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    // Initialize observability via spindle-obs (single source of truth)
    let obs_config = spindle_obs::Config::from_env("operational");
    spindle_obs::init(&obs_config);
    let api_url = resolve_api_url(&cli);
    let state = AppState::new(api_url.clone());

    let addr: SocketAddr = SocketAddr::from(([0, 0, 0, 0], cli.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("spindle-dashboard listening on http://{addr}");
    tracing::info!("proxying Spindle API at {api_url}");

    axum::serve(listener, router(state)).await?;
    Ok(())
}

//! spindle-dashboard — single-binary web dashboard for the Spindle REST API.
//!
//! Serves the embedded React SPA (frontend/dist, compiled in via rust-embed)
//! and reverse-proxies /v1/* to the Spindle API, forwarding the caller's
//! X-Api-Token / Authorization header. One artifact, one port:
//!
//!   spindle-dashboard --api-url http://192.0.2.10:8080 [--port 3000]
//!
//! The API base URL can also be supplied via the `SPINDLE_API_URL` env var.
//! The process holds no session state (tokens live in the browser's
//! localStorage), so N instances can be load-balanced behind Apache / nginx /
//! HAProxy.

mod web;

use axum::routing::any;
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
    about = "Embedded-SPA dashboard + API proxy for the Spindle REST API"
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

fn router(state: AppState) -> axum::Router {
    axum::Router::new()
        // /v1/* → Spindle API (auth headers forwarded verbatim).
        .route("/v1/*path", any(web::proxy_v1))
        .route("/v1", any(web::proxy_v1))
        .merge(web::routes())
        .with_state(state)
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Initialize observability via spindle-obs (single source of truth)
    let obs_config = spindle_obs::Config::from_env("operational");
    spindle_obs::init(&obs_config);
    let cli = Cli::parse();
    let api_url = resolve_api_url(&cli);
    let state = AppState::new(api_url.clone());

    let addr: SocketAddr = SocketAddr::from(([0, 0, 0, 0], cli.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("spindle-dashboard listening on http://{addr}");
    tracing::info!("proxying Spindle API at {api_url}");

    axum::serve(listener, router(state)).await?;
    Ok(())
}

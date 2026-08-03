/// Corpus capture proxy — transparent reverse proxy for Chef Infra Client data collector traffic.
///
/// Usage: spindle-corpus-capture --upstream <url> [--listen <addr>] [--output <dir>]

mod config;
mod metadata;
mod recorder;
mod proxy;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{routing::any, Router};
use clap::Parser;
use tracing::{error, info};

#[tokio::main]
async fn main() {
    // Parse CLI arguments
    let cfg = config::Config::try_parse().unwrap_or_else(|e| {
        eprintln!("CLI error: {}", e);
        std::process::exit(1);
    });

    // Validate configuration
    if let Err(e) = cfg.validate() {
        eprintln!("Configuration error: {}", e);
        std::process::exit(1);
    }

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(&cfg.log_level)
        .init();

    info!(listen = %cfg.listen, upstream = %cfg.get_upstream(), "Starting corpus capture proxy");

    // Create recorder
    let recorder = Arc::new(recorder::Recorder::new(&cfg.output));

    // Create proxy wrapped in Arc
    let proxy = Arc::new(proxy::Proxy::new(&cfg, Arc::clone(&recorder)));

    // Build router — catch-all route for all methods
    let app = Router::new()
        .route("/{*tail}", any(proxy_handler))
        .with_state(Arc::clone(&proxy));

    // Parse listen address
    let addr: SocketAddr = cfg.listen.parse().unwrap_or_else(|e| {
        eprintln!("Invalid listen address {}: {}", cfg.listen, e);
        std::process::exit(1);
    });

    info!("Listening on {}", addr);

    // Store config for shutdown handler
    let upstream_url = cfg.get_upstream().to_string();
    let recorder_for_shutdown = Arc::clone(&recorder);

    // Bind listener
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| {
        eprintln!("Failed to bind {}: {}", addr, e);
        std::process::exit(1);
    });

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            info!("Shutting down, writing corpus metadata...");
            recorder_for_shutdown.write_meta_json("0.1.0", &upstream_url).await;
            info!("Corpus capture complete.");
        })
        .await
        .unwrap_or_else(|e| {
            error!("Server error: {}", e);
            std::process::exit(1);
        });
}

/// Axum handler — routes all requests to the proxy
async fn proxy_handler(
    state: axum::extract::State<Arc<proxy::Proxy>>,
    req: axum::extract::Request,
) -> Result<axum::response::Response, (axum::http::StatusCode, String)> {
    match state.handle(req).await {
        Ok(resp) => Ok(resp),
        Err(e) => Err((axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// Handle graceful shutdown (SIGTERM / Ctrl+C)
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    let term = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to create SIGTERM handler")
            .recv()
            .await;
    };
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}

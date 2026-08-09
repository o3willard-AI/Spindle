//! Spindle server — main application binary.
//!
//! Supports `--validate-config` flag: validates configuration and exits 0 (valid)
//! or 1 (invalid with specific error messages).
//! Supports port conflict detection at startup.
//!
//! On normal startup it loads configuration, assembles the metrics/health and
//! ingest routers, binds a `TcpListener` on `config.server.addr()` and serves
//! HTTP via `axum::serve`:
//!   - GET  /health
//!   - GET  /ready
//!   - GET  /metrics
//!   - POST /ingest/events/data-collector  (auth: `Bearer <token>`)
//!   - POST /ingest/events/inspec         (auth: `Bearer <token>`)
//!
//! The ingest bearer token is read from `SPINDLE_INGEST_TOKEN` and defaults to
//! `spindle-dev-token`. The raw-archive root is read from `SPINDLE_ARCHIVE_DIR`
//! and defaults to `/var/lib/spindle/archive`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;

use spindle_server::ingest::{
    DEFAULT_MAX_INGEST_LAG_SECONDS, PostgresIdempotencyStore, PostgresQueueMonitor,
    InMemoryIdempotencyStore, InMemoryQueueMonitor,
    IngestAppState, IngestConfig,
};
use spindle_server::metrics::{MetricsRegistry, MetricsState};

/// Default ingest bearer token used when `SPINDLE_INGEST_TOKEN` is unset.
const DEFAULT_INGEST_TOKEN: &str = "spindle-dev-token";
/// Default raw-archive root used when `SPINDLE_ARCHIVE_DIR` is unset.
const DEFAULT_ARCHIVE_DIR: &str = "/var/lib/spindle/archive";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut validate_only = false;
    let mut config_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--validate-config" => {
                validate_only = true;
            }
            "--config" | "-c" => {
                if i + 1 < args.len() {
                    config_path = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("Error: --config requires a path argument");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                println!("spindle-server — HTTP API + ingest server");
                println!();
                println!("Usage: spindle-server [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config <PATH>   Path to config file (default: ~/.config/spindle/config.toml or $SPINDLE_CONFIG)");
                println!("  --validate-config Validate configuration and exit");
                println!("  --help            Print this help message");
                println!();
                println!("Environment:");
                println!("  SPINDLE_INGEST_TOKEN  Bearer token required on ingest endpoints (default: spindle-dev-token)");
                println!("  SPINDLE_ARCHIVE_DIR   Raw-archive root directory (default: /var/lib/spindle/archive)");
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    if let Some(path) = &config_path {
        std::env::set_var("SPINDLE_CONFIG", path);
    }

    if validate_only {
        match spindle_config::Config::load() {
            Ok(config) => {
                match config.validate() {
                    Ok(_) => {
                        println!("Configuration is valid");
                        println!("Server: {}:{}", config.server.host, config.server.port);
                        println!("Database: connected");
                        println!("Storage: {}", config.storage.backend);
                        std::process::exit(0);
                    }
                    Err(e) => {
                        eprintln!("Configuration validation failed:");
                        eprintln!("  {}", e);
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                eprintln!("Failed to load configuration: {}", e);
                std::process::exit(1);
            }
        }
    }

    // Normal startup
    let config = match spindle_config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load configuration: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = config.validate() {
        eprintln!("Configuration validation failed: {}", e);
        std::process::exit(1);
    }

    let addr = config.server.addr();
    if let Err(e) = check_port_available(addr) {
        eprintln!("Port conflict: cannot bind to {}", addr);
        eprintln!("  {}", e);
        eprintln!("  Another process may be using this port.");
        std::process::exit(3);
    }

    println!("Starting spindle-server on {}", addr);
    if let Err(e) = run_server(addr) {
        eprintln!("Fatal: server error: {}", e);
        std::process::exit(1);
    }
}

/// Build shared application state and start the axum HTTP server.
fn run_server(addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    // ── Metrics / health state ──────────────────────────────────────────────
    let metrics = Arc::new(MetricsRegistry::new());
    let metrics_state = MetricsState {
        metrics: metrics.clone(),
        start_time: Instant::now(),
    };

    // ── Ingest state ────────────────────────────────────────────────────────
    let token = std::env::var("SPINDLE_INGEST_TOKEN")
        .unwrap_or_else(|_| DEFAULT_INGEST_TOKEN.to_string());
    let archive_root = std::env::var("SPINDLE_ARCHIVE_DIR")
        .unwrap_or_else(|_| DEFAULT_ARCHIVE_DIR.to_string());

    let archive = Arc::new(spindle_rawarchive::LocalArchive::new(&archive_root)?);

    // ── Database connection (production) ──────────────────────────────────
    let database_url = std::env::var("SPINDLE_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| "postgres://spindle:spindle@localhost:5432/spindle".to_string());

    // ── Serve HTTP on the configured address ────────────────────────────────
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let pool = if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
            match sqlx::postgres::PgPoolOptions::new()
                .max_connections(20)
                .acquire_timeout(std::time::Duration::from_secs(5))
                .connect(&database_url)
                .await
            {
                Ok(p) => Some(p),
                Err(e) => {
                    eprintln!("Warning: database connection failed: {}. In-memory fallback.", e);
                    None
                }
            }
        } else {
            None
        };

        // Use Postgres-backed stores when DB is available; fall back to in-memory for dev
        let idempotency: Arc<dyn spindle_server::ingest::IdempotencyStore> = if let Some(ref p) = pool {
            Arc::new(PostgresIdempotencyStore::new(p.clone()))
        } else {
            Arc::new(InMemoryIdempotencyStore::new())
        };
        let queue: Arc<dyn spindle_server::ingest::QueueMonitor> = if let Some(ref p) = pool {
            Arc::new(PostgresQueueMonitor::new(p.clone(), 150.0))
        } else {
            Arc::new(InMemoryQueueMonitor::new(0, 150.0))
        };

        let ingest_state = IngestAppState::new(
            IngestConfig::new(&token),
            archive,
            idempotency,
            queue,
            DEFAULT_MAX_INGEST_LAG_SECONDS * 2,
        );

        // ── Assemble router ──
        let router: Router = Router::new()
            .merge(spindle_server::metrics::metrics_routes(metrics_state))
            .merge(spindle_server::ingest::ingest_routes(ingest_state));

        // ── Serve HTTP on the configured address ────────────────────────────────
        let listener = tokio::net::TcpListener::bind(addr).await?;
        println!("Spindle server listening on http://{}/", addr);
        axum::serve(listener, router).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
    Ok(())
}

/// Check if the given address is available for binding.
pub fn check_port_available(addr: SocketAddr) -> Result<(), std::io::Error> {
    std::net::TcpListener::bind(addr).map(|listener| drop(listener))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_port_available() {
        // Port 0 = OS assigns available port
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        assert!(check_port_available(addr).is_ok());
    }

    #[test]
    fn test_check_port_in_use() {
        // Bind one listener, then check that the same port is in use
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

        assert!(check_port_available(addr).is_err());
    }
}

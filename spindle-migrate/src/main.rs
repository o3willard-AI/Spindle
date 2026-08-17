//! Spindle database migration runner — binary entry point.
//!
//! ## Usage
//!
//! ```sh
//! SPINDLE_DATABASE_URL=postgres://spindle:pw@db:5432/spindle \
//!   ./spindle-migrate --migrations-dir ./migrations
//! ```
//!
//! Runs all forward-only migrations from the `migrations/` directory.

use std::path::PathBuf;

use clap::Parser;

/// Database migration runner for Spindle.
#[derive(Parser, Debug)]
#[command(
    name = "spindle-migrate",
    version,
    about = "Database migration runner for Spindle"
)]
struct Cli {
    /// PostgreSQL connection URL (default: $DATABASE_URL or $SPINDLE_DATABASE_URL)
    #[arg(long, env = "SPINDLE_DATABASE_URL")]
    database_url: Option<String>,

    /// Directory containing migration subdirectories (default: ./migrations)
    #[arg(long, default_value = "migrations")]
    migrations_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Initialize observability via spindle-obs (single source of truth)
    let obs_config = spindle_obs::Config::from_env("operational");
    spindle_obs::init(&obs_config);

    // Resolve database URL: CLI flag > SPINDLE_DATABASE_URL > DATABASE_URL > error
    let db_url = cli.database_url.as_ref().cloned().or_else(|| {
        std::env::var("DATABASE_URL").ok()
    }).ok_or_else(|| {
        "No database URL provided. Use --database-url or set SPINDLE_DATABASE_URL / DATABASE_URL.".to_string()
    })?;

    tracing::info!("Starting Spindle migration runner");
    tracing::info!("Database: {}", db_url);
    tracing::info!("Migrations dir: {}", cli.migrations_dir.display());

    let runner = spindle_migrate::MigrationRunner::new(
        &db_url,
        Some(cli.migrations_dir.to_str().unwrap_or("migrations")),
    );

    match runner.migrate_all().await {
        Ok(_) => {
            tracing::info!("All migrations applied successfully");
            Ok(())
        }
        Err(e) => {
            tracing::error!("Migration failed: {}", e);
            Err(format!("Migration failed: {}", e).into())
        }
    }
}

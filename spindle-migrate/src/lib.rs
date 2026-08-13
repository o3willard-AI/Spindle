//! Database migration runner for Spindle.
//!
//! # Usage
//! ```ignore
//! use spindle_migrate::MigrationRunner;
//!
//! let runner = MigrationRunner::new("postgres://localhost/spindle", None);
//! runner.migrate_all().await?;
//! ```
//!
//! ## Forward-only migrations
//! Migrations are forward-only (no rollback — replay from archive instead).
//! Each migration has an `up.sql` file with the schema changes.

#![allow(warnings)]
use tracing::info;

use sqlx::postgres::PgPool;
use sqlx::Row;

/// Schema version table for tracking applied migrations.
pub struct SchemaVersionTable {
    pub name: &'static str,
}

impl SchemaVersionTable {
    /// Create a new schema version table.
    pub fn new(name: &'static str) -> Self {
        Self { name }
    }

    /// Create the schema version table if it doesn't exist.
    pub async fn create_if_not_exists(&self, pool: &PgPool) -> Result<(), sqlx::Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS schema_versions (
                id BIGSERIAL PRIMARY KEY,
                schema_version BIGINT NOT NULL,
                applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                UNIQUE (schema_version)
            )",
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Get the current schema version.
    pub async fn get_current_version(&self, pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
        let result =
            sqlx::query("SELECT schema_version FROM schema_versions ORDER BY id DESC LIMIT 1")
                .fetch_optional(pool)
                .await?;

        match result {
            Some(row) => Ok(Some(row.get("schema_version"))),
            None => Ok(None),
        }
    }

    /// Record that a migration has been applied.
    pub async fn record_version(&self, pool: &PgPool, version: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO schema_versions (schema_version) VALUES ($1)
             ON CONFLICT (schema_version) DO NOTHING",
        )
        .bind(version)
        .execute(pool)
        .await?;
        Ok(())
    }
}

/// Represents a database migration.
#[derive(Debug, Clone)]
pub struct Migration {
    /// Migration name (e.g., "001_create_schema_version_table").
    pub name: String,
    /// Path to the migration directory.
    pub path: std::path::PathBuf,
}

/// Migration runner that applies forward-only migrations.
pub struct MigrationRunner {
    /// Database URL for connection.
    pub db_url: String,
    /// Optional migrations directory path.
    pub migrations_dir: Option<std::path::PathBuf>,
}

impl MigrationRunner {
    /// Create a new migration runner.
    pub fn new(db_url: &str, migrations_dir: Option<&str>) -> Self {
        Self {
            db_url: db_url.to_string(),
            migrations_dir: migrations_dir.map(|s| s.into()),
        }
    }

    /// Discover all migrations in the migrations directory.
    pub async fn discover_migrations(&self) -> Result<Vec<Migration>, sqlx::Error> {
        let migrations_dir = self
            .migrations_dir
            .as_ref()
            .ok_or_else(|| sqlx::Error::ColumnNotFound("migrations_dir not set".to_string()))?;

        if !migrations_dir.exists() {
            return Ok(vec![]);
        }

        let entries = std::fs::read_dir(migrations_dir)?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.is_dir() {
                    Some(path)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        let migrations: Vec<Migration> = entries
            .into_iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();

                Migration { name, path }
            })
            .collect();

        Ok(migrations)
    }

    /// Get the current schema version from the database.
    pub async fn get_current_schema_version(&self) -> Result<Option<i64>, sqlx::Error> {
        let pool = PgPool::connect(&self.db_url).await?;
        let schema_version_table = SchemaVersionTable::new("schema_versions");
        let version = schema_version_table.get_current_version(&pool).await?;
        Ok(version)
    }

    /// Apply all pending migrations.
    pub async fn migrate_all(&self) -> Result<(), sqlx::Error> {
        let migrations = self.discover_migrations().await?;
        let current_version = self.get_current_schema_version().await?;

        if migrations.is_empty() {
            info!("No migrations to apply");
            return Ok(());
        }

        // Sort migrations by name (zero-padded) for proper ordering
        let migrations = self.sort_migrations(&migrations)?;

        // Find pending migrations
        let current = current_version.unwrap_or(0);
        let pending: Vec<&Migration> = migrations
            .iter()
            .filter(|m| {
                let version = self.extract_version(&m.name);
                version > current
            })
            .collect();

        if pending.is_empty() {
            info!("No pending migrations");
            return Ok(());
        }

        info!("Applying {} pending migrations", pending.len());

        let pool = PgPool::connect(&self.db_url).await?;

        for migration in &pending {
            let version = self.extract_version(&migration.name);
            info!("Applying migration: {}", migration.name);

            // Read and execute the up.sql file
            let up_sql = std::fs::read_to_string(migration.path.join("up.sql"))?;
            sqlx::query(&up_sql).execute(&pool).await?;

            // Record the version
            let schema_version_table = SchemaVersionTable::new("schema_versions");
            schema_version_table.record_version(&pool, version).await?;

            info!("Migration applied: {}", migration.name);
        }

        info!("All migrations applied successfully");
        Ok(())
    }

    /// Sort migrations by name (zero-padded).
    fn sort_migrations(&self, migrations: &Vec<Migration>) -> Result<Vec<Migration>, sqlx::Error> {
        let mut sorted = migrations.clone();
        sorted.sort_by(|a, b| {
            self.extract_version(&a.name)
                .cmp(&self.extract_version(&b.name))
        });
        Ok(sorted)
    }

    /// Extract version number from migration name.
    fn extract_version(&self, name: &str) -> i64 {
        // Extract the numeric prefix from the migration name
        // e.g., "001_create_schema_version_table" -> 1
        name.chars()
            .filter(|&c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<i64>()
            .unwrap_or(0)
    }
}

/// Create the schema version table if it doesn't exist.
pub async fn ensure_schema_version_table(pool: &PgPool) -> Result<(), sqlx::Error> {
    let schema_version_table = SchemaVersionTable::new("schema_versions");
    schema_version_table.create_if_not_exists(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_version() {
        let runner = MigrationRunner::new("postgres://localhost/test", None);
        assert_eq!(runner.extract_version("001_create_schema_version_table"), 1);
        assert_eq!(runner.extract_version("002_create_nodes_table"), 2);
        assert_eq!(runner.extract_version("999_final_migration"), 999);
        assert_eq!(runner.extract_version("invalid_name"), 0);
    }

    #[test]
    fn test_sort_migrations() {
        let runner = MigrationRunner::new("postgres://localhost/test", None);
        let migrations = vec![
            Migration {
                name: "003_create_audit_log".to_string(),
                path: std::path::PathBuf::from("/tmp/migrations/003_create_audit_log"),
            },
            Migration {
                name: "001_create_schema_version_table".to_string(),
                path: std::path::PathBuf::from("/tmp/migrations/001_create_schema_version_table"),
            },
            Migration {
                name: "002_create_nodes_table".to_string(),
                path: std::path::PathBuf::from("/tmp/migrations/002_create_nodes_table"),
            },
        ];

        let sorted = runner.sort_migrations(&migrations).unwrap();
        assert_eq!(sorted[0].name, "001_create_schema_version_table");
        assert_eq!(sorted[1].name, "002_create_nodes_table");
        assert_eq!(sorted[2].name, "003_create_audit_log");
    }
}

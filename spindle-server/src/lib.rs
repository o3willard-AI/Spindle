//! Spindle server — main application binary.
//! Handles HTTP endpoints, configuration, and orchestration.

pub mod ingest;
pub mod runs;
pub mod cookbooks;
pub mod health;
pub mod nodes;
pub mod resource_events;
pub mod waivers;
pub mod compliance;
pub mod authz;
pub mod saml;
pub mod sessions;
pub mod tokens;
pub mod metrics;
pub mod jwk;

use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Discover migrations in a directory.
pub fn discover_migrations(migrations_dir: &Path) -> Vec<Migration> {
    fs::read_dir(migrations_dir)
        .unwrap_or_else(|_| panic!("Failed to read migrations directory: {:?}", migrations_dir))
        .filter_map(|entry| {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                Some(path)
            } else {
                None
            }
        })
        .map(|path| Migration {
            name: path.file_name().unwrap().to_str().unwrap().to_string(),
            path,
        })
        .collect()
}

#[derive(Debug)]
pub struct Migration {
    pub name: String,
    pub path: std::path::PathBuf,
}

#[test]
fn test_discover_migrations() {
    let temp_dir = TempDir::new().unwrap();
    let migrations_dir = temp_dir.path().join("migrations");
    fs::create_dir_all(&migrations_dir).unwrap();

    // Create a test migration
    let migration_dir = migrations_dir.join("001_test");
    fs::create_dir_all(&migration_dir).unwrap();
    fs::write(migration_dir.join("up.sql"), "CREATE TABLE test (id SERIAL PRIMARY KEY);").unwrap();

    let migrations = discover_migrations(&migrations_dir);
    assert_eq!(migrations.len(), 1);
    assert_eq!(migrations[0].name, "001_test");
}

// ── Port conflict detection (M5-04) ────────────────────────────────────────────

use std::net::{SocketAddr, TcpListener};

/// Check if the given address is available for binding.
pub fn check_port_available(addr: SocketAddr) -> Result<(), std::io::Error> {
    TcpListener::bind(addr).map(|listener| drop(listener))
}

#[cfg(test)]
mod port_tests {
    use super::*;

    #[test]
    fn test_port_available() {
        // Port 0 = OS assigns available port
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        assert!(check_port_available(addr).is_ok());
    }

    #[test]
    fn test_port_conflict_detected() {
        // Bind one listener, then check that the same port is in use
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

        assert!(check_port_available(addr).is_err());
    }
}

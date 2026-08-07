//! spindle-rawarchive: Raw payload archive interface + S3 + local FS backends.
//!
//! Per PLANS.md M1-01:
//! - `Archive` trait: store, retrieve, exists, delete, list
//! - S3 backend: configurable endpoint, region, path-style access
//! - Local FS backend: directory-per-date, atomic writes
//! - Keys: `{date}/{digest}.json.gz`
//! - Metadata: receipt timestamp, source token identity, content type

pub mod metadata;

pub use metadata::ArchiveMetadata;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info, warn};

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("Write failed: {0}")]
    WriteFailed(String),
    #[error("Read failed: {0}")]
    ReadFailed(String),
    #[error("Key not found: {0}")]
    NotFound(String),
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Path traversal attempt: {0}")]
    PathTraversal(String),
}

pub type Result<T> = std::result::Result<T, ArchiveError>;

// ── Metadata ────────────────────────────────────────────────────────────────

/// Payload metadata stored with every archived payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayloadMetadata {
    pub receipt_timestamp: chrono::DateTime<chrono::Utc>,
    pub source_token: String,
    pub content_type: String,
    pub payload_size: u64,
    pub payload_sha256: String,
}

impl PayloadMetadata {
    pub fn new(
        receipt_timestamp: chrono::DateTime<chrono::Utc>,
        source_token: String,
        content_type: String,
        payload: &[u8],
    ) -> Self {
        use sha2::{Sha256, Digest};
        let hash = {
            let mut hasher = Sha256::new();
            hasher.update(payload);
            hasher.finalize()
        };
        Self {
            receipt_timestamp,
            source_token,
            content_type,
            payload_size: payload.len() as u64,
            payload_sha256: hex::encode(hash),
        }
    }
}

// ── Archive trait ───────────────────────────────────────────────────────────

/// The raw archive interface. Every payload goes through this.
pub trait Archive: Send + Sync + Debug {
    /// Store a payload with metadata. Returns a reference key for later retrieval.
    /// Key format: `{date}/{digest}.json.gz`
    fn store(
        &self,
        payload: &[u8],
        metadata: &ArchiveMetadata,
    ) -> Result<String>;

    /// Retrieve payload by key. Returns raw bytes.
    fn retrieve(&self, key: &str) -> Result<Vec<u8>>;

    /// Check if a key exists.
    fn exists(&self, key: &str) -> Result<bool>;

    /// Delete a key.
    fn delete(&self, key: &str) -> Result<()>;

    /// List keys in a time range.
    fn list(&self, time_range: Option<std::ops::Range<chrono::DateTime<chrono::Utc>>>) -> Result<Vec<String>>;

    /// Get a reference to the underlying storage for advanced operations.
    fn storage(&self) -> Arc<dyn Storage>;
}

/// Trait for low-level storage operations.
pub trait Storage: Send + Sync + Debug {
    fn put(&self, key: &str, data: &[u8]) -> Result<()>;
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn delete(&self, key: &str) -> Result<bool>;
    fn exists(&self, key: &str) -> Result<bool>;
    fn list_prefix(&self, prefix: &str) -> Result<Vec<String>>;
    fn list(&self) -> Result<Vec<String>>;
    fn capacity(&self) -> Result<u64>;
}

// ── S3 Backend (placeholder) ────────────────────────────────────────────────

/// S3-compatible archive backend using `object_store`.
/// TODO: Implement with object_store v0.9
pub struct S3Archive {
    // placeholder
}

impl S3Archive {
    pub fn new(endpoint: &str, _region: &str) -> Self {
        info!("S3Archive created (endpoint: {})", endpoint);
        S3Archive {}
    }
}

impl Default for S3Archive {
    fn default() -> Self {
        Self::new("http://localhost:9000", "us-east-1")
    }
}

// TODO: Implement Archive trait for S3Archive
// TODO: Implement Storage trait for S3Adapter

// ── Local FS Backend ────────────────────────────────────────────────────────

/// Local filesystem archive backend.
#[derive(Debug)]
pub struct LocalArchive {
    root: String,
    storage: Arc<dyn Storage>,
}

impl LocalArchive {
    /// Create a new local archive. `root` is the directory where data is stored.
    pub fn new(root: &str) -> Result<Self> {
        std::fs::create_dir_all(root).map_err(|e| ArchiveError::Io(e))?;
        info!(root = %root, "LocalArchive created");

        let storage = Arc::new(LocalStorageAdapter {
            root: root.to_string(),
        });

        Ok(LocalArchive {
            root: root.to_string(),
            storage,
        })
    }
}

impl Archive for LocalArchive {
    fn store(&self, payload: &[u8], metadata: &ArchiveMetadata) -> Result<String> {
        let key = build_key(&metadata.date_str(), &metadata.payload_sha256)?;

        // Write payload with atomic rename
        let payload_path = format!("{}/{}", self.root, key);
        let payload_dir = std::path::Path::new(&payload_path).parent().unwrap();
        std::fs::create_dir_all(payload_dir).map_err(|e| ArchiveError::Io(e))?;

        let temp_path = format!("{}.tmp", payload_path);
        std::fs::write(&temp_path, payload).map_err(|e| ArchiveError::Io(e))?;
        std::fs::rename(&temp_path, &payload_path)
            .map_err(|e| ArchiveError::Io(e))?;

        // Write metadata
        let meta_path = format!("{}/{}", self.root, format!("{}.meta", key));
        let meta_bytes = serde_json::to_vec(&metadata)
            .map_err(|e| ArchiveError::Serialization(e.to_string()))?;
        let temp_meta = format!("{}.tmp", meta_path);
        std::fs::write(&temp_meta, &meta_bytes).map_err(|e| ArchiveError::Io(e))?;
        std::fs::rename(&temp_meta, &meta_path).map_err(|e| ArchiveError::Io(e))?;

        info!(key = %key, "Payload stored locally");
        Ok(key)
    }

    fn retrieve(&self, key: &str) -> Result<Vec<u8>> {
        let payload_path = format!("{}/{}", self.root, key);
        match std::fs::read(&payload_path) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(ArchiveError::NotFound(key.to_string()))
            }
            Err(e) => Err(ArchiveError::Io(e)),
        }
    }

    fn exists(&self, key: &str) -> Result<bool> {
        let payload_path = format!("{}/{}", self.root, key);
        Ok(std::fs::metadata(&payload_path).is_ok())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let payload_path = format!("{}/{}", self.root, key);
        let meta_path = format!("{}/{}", self.root, format!("{}.meta", key));

        std::fs::remove_file(&payload_path).ok();
        std::fs::remove_file(&meta_path).ok();

        info!(key = %key, "Payload deleted locally");
        Ok(())
    }

    fn list(&self, time_range: Option<std::ops::Range<chrono::DateTime<chrono::Utc>>>) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let entries: Vec<_> = std::fs::read_dir(&self.root).map_err(|e| ArchiveError::Io(e))?
            .filter_map(|e| e.ok())
            .collect();

        for entry in entries {
            let path = entry.path();
            let file_name = path.file_name().unwrap().to_string_lossy().to_string();

            // Skip metadata files
            if file_name.ends_with(".meta") {
                continue;
            }

            keys.push(file_name.clone());

            // Filter by time range if provided
            if let Some(ref range) = time_range {
                let meta_path = format!("{}/{}", self.root, format!("{}.meta", file_name));
                if let Ok(meta_bytes) = std::fs::read(&meta_path) {
                    if let Ok(meta) = serde_json::from_slice::<ArchiveMetadata>(&meta_bytes) {
                        let ts = meta.receipt_timestamp.to_utc();
                        if ts < range.start || ts >= range.end {
                            keys.pop();
                        }
                    }
                }
            }
        }

        Ok(keys)
    }

    fn storage(&self) -> Arc<dyn Storage> {
        self.storage.clone()
    }
}

/// Local filesystem storage adapter.
#[derive(Debug)]
pub struct LocalStorageAdapter {
    root: String,
}

impl Storage for LocalStorageAdapter {
    fn put(&self, key: &str, data: &[u8]) -> Result<()> {
        let path = format!("{}/{}", self.root, key);
        let dir = std::path::Path::new(&path).parent().unwrap();
        std::fs::create_dir_all(dir).map_err(|e| ArchiveError::Io(e))?;
        std::fs::write(&path, data).map_err(|e| ArchiveError::Io(e))
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let path = format!("{}/{}", self.root, key);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ArchiveError::Io(e)),
        }
    }

    fn delete(&self, key: &str) -> Result<bool> {
        let path = format!("{}/{}", self.root, key);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(ArchiveError::Io(e)),
        }
    }

    fn exists(&self, key: &str) -> Result<bool> {
        let path = format!("{}/{}", self.root, key);
        Ok(std::fs::metadata(&path).is_ok())
    }

    fn list_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let prefix_path = format!("{}/{}", self.root, prefix);
        let entries: Vec<_> = std::fs::read_dir(&prefix_path).map_err(|e| ArchiveError::Io(e))?
            .filter_map(|e| e.ok())
            .collect();

        for entry in entries {
            let path = entry.path();
            let file_name = path.file_name().unwrap().to_string_lossy().to_string();

            // Skip metadata files
            if file_name.ends_with(".meta") {
                continue;
            }

            keys.push(file_name);
        }

        Ok(keys)
    }

    fn list(&self) -> Result<Vec<String>> {
        self.list_prefix("")
    }

    fn capacity(&self) -> Result<u64> {
        let dir = std::path::Path::new(&self.root);
        if dir.exists() {
            let metadata = std::fs::metadata(dir).map_err(|e| ArchiveError::Io(e))?;
            Ok(metadata.len())
        } else {
            Ok(0)
        }
    }
}

// ── Key generation ──────────────────────────────────────────────────────────

fn build_key(date: &str, digest: &str) -> Result<String> {
    Ok(format!("{}/{}.json.gz", date, digest))
}

// ── Re-exports ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn test_local_archive_basic() -> Result<()> {
        let tmp_dir = std::env::temp_dir()
            .join(format!("spindle_test_{}", chrono::Utc::now().timestamp()));
        let archive = LocalArchive::new(tmp_dir.to_str().unwrap())?;

        let metadata = ArchiveMetadata::new(
            "test_digest_abc123".to_string(),
            "application/json".to_string(),
            "test_token".to_string(),
            Utc::now(),
        );

        let key = archive.store(b"test payload", &metadata)?;
        assert!(archive.exists(&key)?);

        let retrieved = archive.retrieve(&key)?;
        assert_eq!(retrieved, b"test payload");

        archive.delete(&key)?;
        assert!(!archive.exists(&key)?);

        Ok(())
    }
}

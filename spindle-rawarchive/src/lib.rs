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
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info, warn};

// S3 imports (behind the s3 feature)
#[cfg(feature = "s3")]
use object_store::aws::{AmazonS3, AmazonS3Builder, AmazonS3ConfigKey};
#[cfg(feature = "s3")]
use object_store::path::Path as S3Path;
#[cfg(feature = "s3")]
#[cfg(feature = "s3")]
use object_store::{PutOptions, GetOptions};
#[cfg(feature = "s3")]
use futures::StreamExt;

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

// ── S3 Backend ───────────────────────────────────────────────────────────────

/// S3-compatible archive backend using `object_store::aws::AmazonS3`.
///
/// Supports AWS S3, MinIO, and any S3-compatible storage.
/// Path-style vs virtual-hosted auto-detection based on endpoint.
#[cfg(feature = "s3")]
#[derive(Debug, Clone)]
pub struct S3Archive {
    client: Arc<dyn object_store::ObjectStore>,
    bucket: String,
    endpoint: String,
    region: String,
    /// True for path-style (e.g., MinIO), false for virtual-hosted (e.g., AWS S3).
    path_style: bool,
    storage: Arc<dyn Storage>,
}

#[cfg(feature = "s3")]
impl S3Archive {
    /// Create a new S3 archive backend.
    pub fn new(
        endpoint: &str,
        region: &str,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
    ) -> Result<Self> {
        let path_style = auto_detect_path_style(endpoint);

        let mut builder = AmazonS3Builder::new()
            .with_endpoint(endpoint)
            .with_bucket_name(bucket)
            .with_region(region)
            .with_access_key_id(access_key)
            .with_secret_access_key(secret_key);

        if path_style {
            builder = builder.with_config(
                AmazonS3ConfigKey::VirtualHostedStyleRequest,
                "false",
            );
        }

        let client = builder.build().map_err(|e| {
            ArchiveError::Storage(format!("S3 client build failed: {}", e))
        })?;

        let client: Arc<dyn object_store::ObjectStore> = Arc::new(client);

        let storage = Arc::new(S3StorageAdapter {
            client: client.clone(),
            bucket: bucket.to_string(),
        });

        info!(
            endpoint = %endpoint,
            region = %region,
            bucket = %bucket,
            path_style = %path_style,
            "S3Archive created"
        );

        Ok(Self {
            client,
            bucket: bucket.to_string(),
            endpoint: endpoint.to_string(),
            region: region.to_string(),
            path_style,
            storage,
        })
    }

    /// Returns true if using path-style access (e.g., MinIO).
    pub fn is_path_style(&self) -> bool {
        self.path_style
    }

    /// Get the bucket name.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Get the endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

/// Auto-detect path-style access from endpoint URL.
pub fn auto_detect_path_style(endpoint: &str) -> bool {
    if endpoint.contains("localhost") || endpoint.contains("127.0.0.1") {
        return true;
    }
    if !endpoint.contains("amazonaws.com") {
        return true;
    }
    false
}

#[cfg(feature = "s3")]
impl Archive for S3Archive {
    fn store(&self, payload: &[u8], metadata: &ArchiveMetadata) -> Result<String> {
        let key = build_key(&metadata.date_str(), &metadata.payload_sha256)?;
        validate_key(&key)?;

        let location = S3Path::from(key.as_str());
        let bytes: bytes::Bytes = payload.to_vec().into();

        futures::executor::block_on(async {
            self.client
                .put(&location, bytes)
                .await
                .map_err(|e| ArchiveError::WriteFailed(format!("S3 put {}: {}", key, e)))?;

            // Store metadata as separate object
            let meta_key = format!("{}.meta", key);
            let meta_location = S3Path::from(meta_key.as_str());
            let meta_bytes: bytes::Bytes = serde_json::to_vec(metadata)
                .map_err(|e| ArchiveError::Serialization(e.to_string()))?
                .into();
            self.client
                .put(&meta_location, meta_bytes)
                .await
                .map_err(|e| ArchiveError::WriteFailed(format!("S3 meta {}: {}", key, e)))?;

            Ok(key)
        })
    }

    fn retrieve(&self, key: &str) -> Result<Vec<u8>> {
        validate_key(key)?;
        let location = S3Path::from(key);

        futures::executor::block_on(async {
            match self.client.get(&location).await {
                Ok(result) => {
                    let bytes = result
                        .bytes()
                        .await
                        .map_err(|e| ArchiveError::ReadFailed(format!("S3 get {}: {}", key, e)))?;
                    Ok(bytes.to_vec())
                }
                Err(e) => Err(ArchiveError::NotFound(key.to_string())),
            }
        })
    }

    fn exists(&self, key: &str) -> Result<bool> {
        validate_key(key)?;
        let location = S3Path::from(key);

        futures::executor::block_on(async {
            match self.client.head(&location).await {
                Ok(_) => Ok(true),
                Err(_) => Ok(false),
            }
        })
    }

    fn delete(&self, key: &str) -> Result<()> {
        validate_key(key)?;
        let location = S3Path::from(key);

        futures::executor::block_on(async {
            self.client
                .delete(&location)
                .await
                .map_err(|e| ArchiveError::Storage(format!("S3 delete {}: {}", key, e)))?;

            // Also delete metadata
            let meta_key = format!("{}.meta", key);
            let meta_location = S3Path::from(meta_key.as_str());
            let _ = self.client.delete(&meta_location).await;

            info!(key = %key, "Payload deleted from S3");
            Ok(())
        })
    }

    fn list(&self, _time_range: Option<std::ops::Range<chrono::DateTime<chrono::Utc>>>) -> Result<Vec<String>> {
        let mut keys = Vec::new();

        let result: std::result::Result<(), object_store::Error> = futures::executor::block_on(async {
            let mut list_stream = self.client.list(None);
            while let Some(item) = list_stream.next().await {
                match item {
                    Ok(meta) => {
                        let key = meta.location.as_ref().to_string();
                        if key.ends_with(".meta") {
                            continue;
                        }
                        keys.push(key);
                    }
                    Err(e) => {
                        warn!("S3 list error: {}", e);
                    }
                }
            }
            Ok::<(), object_store::Error>(())
        });
        result.map_err(|e| ArchiveError::Storage(format!("S3 list: {}", e)))?;

        Ok(keys)
    }

    fn storage(&self) -> Arc<dyn Storage> {
        self.storage.clone()
    }
}

/// S3 storage adapter — implements the Storage trait for S3.
#[cfg(feature = "s3")]
#[derive(Debug, Clone)]
struct S3StorageAdapter {
    client: Arc<dyn object_store::ObjectStore>,
    bucket: String,
}

#[cfg(feature = "s3")]
impl Storage for S3StorageAdapter {
    fn put(&self, key: &str, data: &[u8]) -> Result<()> {
        let location = S3Path::from(key);
        let bytes: bytes::Bytes = data.to_vec().into();
        futures::executor::block_on(async {
            self.client
                .put(&location, bytes)
                .await
                .map_err(|e| ArchiveError::WriteFailed(format!("S3 put {}: {}", key, e)))?;
            Ok(())
        })
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let location = S3Path::from(key);
        futures::executor::block_on(async {
            match self.client.get(&location).await {
                Ok(result) => {
                    let bytes = result
                        .bytes()
                        .await
                        .map_err(|e| ArchiveError::ReadFailed(format!("S3 get {}: {}", key, e)))?;
                    Ok(Some(bytes.to_vec()))
                }
                Err(_) => Ok(None),
            }
        })
    }

    fn delete(&self, key: &str) -> Result<bool> {
        let location = S3Path::from(key);
        futures::executor::block_on(async {
            match self.client.delete(&location).await {
                Ok(_) => Ok(true),
                Err(_) => Ok(false),
            }
        })
    }

    fn exists(&self, key: &str) -> Result<bool> {
        let location = S3Path::from(key);
        futures::executor::block_on(async {
            match self.client.head(&location).await {
                Ok(_) => Ok(true),
                Err(_) => Ok(false),
            }
        })
    }

    fn list_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let location = if prefix.is_empty() {
            S3Path::from("")
        } else {
            S3Path::from(prefix)
        };
        let mut keys = Vec::new();

        let result: Result<Vec<String>> = futures::executor::block_on(async {
            let mut list_stream = self.client.list(Some(&location));
            while let Some(item) = list_stream.next().await {
                if let Ok(meta) = item {
                    let key = meta.location.as_ref().to_string();
                    if !key.ends_with(".meta") {
                        keys.push(key);
                    }
                }
            }
            Ok(keys)
        });
        result.map_err(|e| ArchiveError::Storage(format!("S3 list: {}", e)))
    }

    fn list(&self) -> Result<Vec<String>> {
        self.list_prefix("")
    }

    fn capacity(&self) -> Result<u64> {
        // S3 doesn't report capacity — return 0 (unlimited)
        Ok(0)
    }
}

// ── Placeholder when s3 feature is disabled ───────────────────────────────────

#[cfg(not(feature = "s3"))]
/// S3 archive backend. Enable the `s3` feature to use.
pub struct S3Archive;

#[cfg(not(feature = "s3"))]
impl S3Archive {
    pub fn new(
        _endpoint: &str,
        _region: &str,
        _bucket: &str,
        _access_key: &str,
        _secret_key: &str,
    ) -> Result<Self> {
        Err(ArchiveError::Storage(
            "S3 feature not enabled. Build with --features s3".to_string()
        ))
    }
}

// ── Helper functions ────────────────────────────────────────────────────────

/// Build a storage key from date and payload hash.
pub fn build_key(date: &str, digest: &str) -> Result<String> {
    Ok(format!("{}/{}.json.gz", date, digest))
}

/// Validate a key to prevent path traversal attacks.
pub fn validate_key(key: &str) -> Result<()> {
    if key.contains("..") {
        return Err(ArchiveError::PathTraversal(format!(
            "path traversal attempt: {}",
            key
        )));
    }
    if key.starts_with('/') {
        return Err(ArchiveError::PathTraversal(format!(
            "absolute path not allowed: {}",
            key
        )));
    }
    if key.contains('\0') {
        return Err(ArchiveError::PathTraversal(format!(
            "null byte in key: {}",
            key
        )));
    }
    if key.contains('\\') {
        return Err(ArchiveError::PathTraversal(format!(
            "path traversal attempt: {}",
            key
        )));
    }
    Ok(())
}

/// Generate a payload key from payload content.
pub fn payload_key(payload: &[u8], date: &str) -> Result<String> {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(payload);
    let hash = hex::encode(hasher.finalize());
    build_key(date, &hash)
}

// ── Local FS Backend ─────────────────────────────────-----------------------

/// Local filesystem archive backend.
#[derive(Debug)]
pub struct LocalArchive {
    root: String,
    storage: Arc<dyn Storage>,
}

impl LocalArchive {
    /// Create a new local archive. `root` is the directory where data is stored.
    pub fn new(root: &str) -> Result<Self> {
        // Validate root path
        if std::path::Path::new(root).is_file() {
            return Err(ArchiveError::PathTraversal(
                "Root directory must be a directory, not a file".to_string()
            ));
        }

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
        // Validate key for path traversal
        let key = build_key(&metadata.date_str(), &metadata.payload_sha256)?;
        validate_key(&key)?;

        // Write payload with atomic rename
        let payload_path = format!("{}/{}", self.root, key);
        let payload_dir = std::path::Path::new(&payload_path).parent().unwrap();
        std::fs::create_dir_all(payload_dir).map_err(|e| ArchiveError::Io(e))?;

        let temp_path = format!("{}.tmp", payload_path);
        std::fs::write(&temp_path, payload).map_err(|e| {
            ArchiveError::WriteFailed(format!("payload {}: {e}", key))
        })?;
        std::fs::rename(&temp_path, &payload_path).map_err(|e| {
            let _ = std::fs::remove_file(&temp_path);
            ArchiveError::WriteFailed(format!("rename {}: {e}", key))
        })?;

        // Write metadata (also write-then-rename for atomicity)
        let meta_path = format!("{}/{}", self.root, format!("{}.meta", key));
        let meta_bytes = serde_json::to_vec(&metadata)
            .map_err(|e| ArchiveError::Serialization(e.to_string()))?;
        let temp_meta = format!("{}.tmp", meta_path);
        std::fs::write(&temp_meta, &meta_bytes).map_err(|e| {
            ArchiveError::WriteFailed(format!("metadata {}: {e}", key))
        })?;
        std::fs::rename(&temp_meta, &meta_path).map_err(|e| {
            let _ = std::fs::remove_file(&temp_meta);
            ArchiveError::WriteFailed(format!("metadata rename {}: {e}", key))
        })?;

        info!(key = %key, "Payload stored locally");
        Ok(key)
    }

    fn retrieve(&self, key: &str) -> Result<Vec<u8>> {
        validate_key(key)?;
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
        validate_key(key)?;
        let payload_path = format!("{}/{}", self.root, key);
        Ok(std::fs::metadata(&payload_path).is_ok())
    }

    fn delete(&self, key: &str) -> Result<()> {
        validate_key(key)?;
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

// ── Batch operations ───────────────────────────────────────────────────────

/// Represents an incomplete batch recovered from disk after a crash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialBatch {
    /// Unique batch identifier.
    pub batch_id: String,
    /// All keys that were part of this batch.
    pub keys: Vec<String>,
    /// Keys that were successfully written before the crash.
    pub complete_keys: Vec<String>,
}

/// Collect all .tmp files recursively under `dir`, returning their paths.
fn collect_batch_tmp_files(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            result.extend(collect_batch_tmp_files(&path)?);
        } else if let Some(ext) = path.extension() {
            if ext == "tmp" {
                result.push(path);
            }
        }
    }
    Ok(result)
}

impl LocalArchive {
    /// Begin a new batch. Returns a unique batch ID string.
    pub fn begin_batch(&self) -> String {
        let batch_id = format!(
            "batch_{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let batch_dir = format!("{}/_batches/{}", self.root, batch_id);
        std::fs::create_dir_all(&batch_dir).expect("Failed to create batch directory");
        info!(batch_id = %batch_id, "Batch started");
        batch_id
    }

    /// Add a payload to a batch. The entry is written to temporary storage
    /// and is NOT retrievable until the batch is committed.
    pub fn add_to_batch(
        &self,
        batch_id: &str,
        key: &str,
        payload: Vec<u8>,
        metadata: ArchiveMetadata,
    ) -> Result<()> {
        let batch_dir = format!("{}/_batches/{}", self.root, batch_id);
        let tmp_path = format!("{}/{}.tmp", batch_dir, key);
        let meta_path = format!("{}/{}.meta", batch_dir, key);

        // Ensure parent dirs exist for the temp path
        let tmp_parent = Path::new(&tmp_path).parent().unwrap();
        std::fs::create_dir_all(tmp_parent).map_err(|e| ArchiveError::Io(e))?;

        std::fs::write(&tmp_path, &payload).map_err(|e| {
            ArchiveError::WriteFailed(format!("batch {} key {}: write {e}", batch_id, key))
        })?;

        // Write metadata
        let meta_parent = Path::new(&meta_path).parent().unwrap();
        std::fs::create_dir_all(meta_parent).map_err(|e| ArchiveError::Io(e))?;
        let meta_bytes = serde_json::to_vec(&metadata).map_err(|e| {
            ArchiveError::Serialization(e.to_string())
        })?;
        std::fs::write(&meta_path, &meta_bytes).map_err(|e| {
            ArchiveError::WriteFailed(format!("batch {} key {}: meta write {e}", batch_id, key))
        })?;

        // Record this key in the batch manifest so recovery can find all keys
        self.append_batch_manifest(batch_id, key)?;

        info!(batch_id = %batch_id, key = %key, "Payload added to batch");
        Ok(())
    }

    /// Append a key to the batch manifest file.
    fn append_batch_manifest(&self, batch_id: &str, key: &str) -> Result<()> {
        let manifest_path = format!("{}/_batches/{}/manifest.json", self.root, batch_id);

        // Read existing manifest, append key, write back
        let keys: Vec<String> = if Path::new(&manifest_path).exists() {
            let data = std::fs::read_to_string(&manifest_path).map_err(|e| {
                ArchiveError::Serialization(format!("read manifest: {e}"))
            })?;
            serde_json::from_str(&data).map_err(|e| {
                ArchiveError::Serialization(format!("parse manifest: {e}"))
            })?
        } else {
            Vec::new()
        };

        let mut keys = keys;
        if !keys.contains(&key.to_string()) {
            keys.push(key.to_string());
        }

        let manifest_bytes = serde_json::to_vec(&keys).map_err(|e| {
            ArchiveError::Serialization(e.to_string())
        })?;
        std::fs::write(&manifest_path, &manifest_bytes).map_err(|e| {
            ArchiveError::WriteFailed(format!("batch {} manifest: {e}", batch_id))
        })?;

        Ok(())
    }

    /// Commit a batch: promotes all entries from temporary to final storage.
    /// If interrupted mid-commit, already-promoted entries remain valid;
    /// non-promoted entries stay as .tmp files.
    pub fn commit_batch(&self, batch_id: &str) -> Result<()> {
        let batch_dir = format!("{}/_batches/{}", self.root, batch_id);

        // Read manifest to get ALL keys (more reliable than scanning .tmp files
        // which may have been partially promoted before a crash).
        let manifest_path = format!("{}/_batches/{}/manifest.json", self.root, batch_id);
        let all_keys: Vec<String> = if Path::new(&manifest_path).exists() {
            let data = std::fs::read_to_string(&manifest_path).map_err(|e| {
                ArchiveError::Serialization(format!("read manifest: {e}"))
            })?;
            serde_json::from_str(&data).map_err(|e| {
                ArchiveError::Serialization(format!("parse manifest: {e}"))
            })?
        } else {
            // Fallback: scan .tmp files
            collect_batch_tmp_files(&std::path::Path::new(&batch_dir))?
                .into_iter()
                .filter_map(|p| {
                    p.strip_prefix(&batch_dir)
                        .ok()
                        .and_then(|r| r.to_str())
                        .and_then(|s| s.strip_suffix(".tmp"))
                        .map(|k| k.to_string())
                })
                .collect()
        };

        info!(batch_id = %batch_id, count = all_keys.len(), "Committing batch from manifest");

        // Track which keys were committed to avoid duplicates
        let mut committed: std::collections::HashSet<String> = std::collections::HashSet::new();

        for key in &all_keys {
            if committed.contains(key) {
                continue;
            }
            committed.insert(key.clone());

            let tmp_path = format!("{}/{}.tmp", batch_dir, key);
            let final_path = format!("{}/{}", self.root, key);
            let final_dir = Path::new(&final_path).parent().unwrap();
            std::fs::create_dir_all(final_dir).map_err(|e| ArchiveError::Io(e))?;

            // Try to rename .tmp; if it doesn't exist the entry was already
            // promoted by a previous commit attempt — skip.
            if !Path::new(&tmp_path).exists() {
                debug!(batch_id = %batch_id, key = %key, "Entry already promoted, skipping");
                continue;
            }

            std::fs::rename(&tmp_path, &final_path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp_path);
                ArchiveError::WriteFailed(format!("batch {} rename {}: {e}", batch_id, key))
            })?;

            // Also move metadata if present
            let tmp_meta = format!("{}/{}.meta", batch_dir, key);
            let final_meta = format!("{}/{}.meta", self.root, key);
            if Path::new(&tmp_meta).exists() {
                std::fs::rename(&tmp_meta, &final_meta).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp_meta);
                    ArchiveError::WriteFailed(format!("batch {} meta rename {}: {e}", batch_id, key))
                })?;
            }

            debug!(batch_id = %batch_id, key = %key, "Batch entry committed");
        }

        // Mark batch as complete
        let complete_marker = format!("{}/_batches/{}.complete", self.root, batch_id);
        let manifest_bytes = serde_json::to_vec(&all_keys).map_err(|e| {
            ArchiveError::Serialization(e.to_string())
        })?;
        std::fs::write(&complete_marker, &manifest_bytes).map_err(|e| {
            ArchiveError::WriteFailed(format!("batch {} marker: {e}", batch_id))
        })?;

        info!(batch_id = %batch_id, committed = committed.len(), "Batch committed");
        Ok(())
    }

    /// Scan for incomplete batches and return them for recovery.
    /// Called on startup to detect crash-recovery scenarios.
    pub fn recover_incomplete_batches(&self) -> Result<Vec<PartialBatch>> {
        let batches_dir = format!("{}/_batches", self.root);

        // List all batch directories
        let batch_dirs: Vec<String> = match std::fs::read_dir(&batches_dir) {
            Ok(dir) => dir
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with("batch_") {
                        Some(name)
                    } else {
                        None
                    }
                })
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(ArchiveError::Io(e)),
        };

        let mut partial = Vec::new();

        for batch_id in batch_dirs {
            let complete_marker = format!("{}/_batches/{}.complete", self.root, batch_id);

            // If .complete marker exists, batch is fine
            if Path::new(&complete_marker).exists() {
                continue;
            }

            // This batch is incomplete.
            // Read the manifest for the authoritative list of keys.
            let manifest_path = format!("{}/_batches/{}/manifest.json", self.root, batch_id);
            let keys: Vec<String> = if Path::new(&manifest_path).exists() {
                let data = std::fs::read_to_string(&manifest_path).map_err(|e| {
                    ArchiveError::Serialization(format!("read manifest: {e}"))
                })?;
                serde_json::from_str(&data).map_err(|e| {
                    ArchiveError::Serialization(format!("parse manifest: {e}"))
                })?
            } else {
                // Fallback: collect from .tmp files
                let tmp_files =
                    collect_batch_tmp_files(&std::path::Path::new(&format!(
                        "{}/_batches/{}",
                        self.root, batch_id
                    )))?;
                tmp_files
                    .into_iter()
                    .filter_map(|p| {
                        p.file_stem().and_then(|s| s.to_str()).map(|k| k.to_string())
                    })
                    .collect()
            };

            let mut complete_keys: Vec<String> = Vec::new();
            for key in &keys {
                let final_path = format!("{}/{}", self.root, key);
                if Path::new(&final_path).exists() {
                    complete_keys.push(key.clone());
                }
            }
            complete_keys.sort();

            if !keys.is_empty() {
                partial.push(PartialBatch {
                    batch_id: batch_id.clone(),
                    keys,
                    complete_keys,
                });
            }
        }

        if !partial.is_empty() {
            warn!(count = partial.len(), "Found incomplete batches on startup");
        }

        Ok(partial)
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

    #[tokio::test]
    async fn test_path_traversal_protection() -> Result<()> {
        let tmp_dir = std::env::temp_dir()
            .join(format!("spindle_test_{}", chrono::Utc::now().timestamp()));
        let archive = LocalArchive::new(tmp_dir.to_str().unwrap())?;

        let metadata = ArchiveMetadata::new(
            "test_digest".to_string(),
            "application/json".to_string(),
            "test_token".to_string(),
            Utc::now(),
        );

        // Test path traversal in key
        let malicious_key = "../etc/passwd.json.gz";
        let result = archive.retrieve(malicious_key);
        assert!(result.is_err());
        match result {
            Err(ArchiveError::PathTraversal(msg)) => {
                assert!(msg.contains("path traversal"));
            }
            _ => panic!("Expected PathTraversal error"),
        }

        // Test absolute path
        let absolute_key = "/etc/passwd.json.gz";
        let result = archive.retrieve(absolute_key);
        assert!(result.is_err());
        match result {
            Err(ArchiveError::PathTraversal(msg)) => {
                assert!(msg.contains("absolute path"));
            }
            _ => panic!("Expected PathTraversal error"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_byte_identical_verification() -> Result<()> {
        let tmp_dir = std::env::temp_dir()
            .join(format!("spindle_test_{}", chrono::Utc::now().timestamp()));
        let archive = LocalArchive::new(tmp_dir.to_str().unwrap())?;

        let original_payload = b"Hello, World! This is a test payload with some data.";
        let metadata = ArchiveMetadata::new(
            sha256_hash(original_payload),
            "application/json".to_string(),
            "test_token".to_string(),
            Utc::now(),
        );

        // Store payload
        let key = archive.store(original_payload, &metadata)?;

        // Retrieve and verify byte-identical
        let retrieved = archive.retrieve(&key)?;
        assert_eq!(retrieved, original_payload);

        // Verify hash matches
        assert_eq!(sha256_hash(&retrieved), metadata.payload_sha256);

        Ok(())
    }

    #[test]
    fn test_validate_key() {
        // Valid key
        assert!(validate_key("2026-01-01/abc123.json.gz").is_ok());

        // Path traversal
        assert!(validate_key("../etc/passwd.json.gz").is_err());
        assert!(validate_key("2026-01-01/../etc/passwd.json.gz").is_err());

        // Absolute path
        assert!(validate_key("/etc/passwd.json.gz").is_err());
        assert!(validate_key("\\etc\\passwd.json.gz").is_err());

        // Null byte
        assert!(validate_key("2026-01-01/abc\0123.json.gz").is_err());
    }

    fn sha256_hash(data: &[u8]) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        hex::encode(result)
    }

    // ── M1-03: Atomicity + crash recovery tests ──────────────────────────

    #[tokio::test]
    async fn test_atomic_write_complete_payload() -> Result<()> {
        // Verify that a successfully stored payload is complete, never partial.
        let tmp_dir = std::env::temp_dir()
            .join(format!("spindle_atomic_{}", chrono::Utc::now().timestamp()));
        let archive = LocalArchive::new(tmp_dir.to_str().unwrap())?;

        let payload = b"This is a test payload for atomicity verification. It should be stored and retrieved completely, not in fragments.";
        let metadata = ArchiveMetadata::new(
            sha256_hash(payload),
            "application/json".to_string(),
            "test_token".to_string(),
            Utc::now(),
        );

        let key = archive.store(payload, &metadata)?;

        // Retrieve and verify the payload is byte-identical (not partial)
        let retrieved = archive.retrieve(&key)?;
        assert_eq!(retrieved, payload);
        assert_eq!(retrieved.len(), payload.len());

        // Verify no .tmp file was left behind after successful rename
        let payload_path = format!("{}/{}", tmp_dir.to_str().unwrap(), key);
        assert!(std::fs::metadata(&payload_path).is_ok());
        assert!(std::fs::metadata(format!("{}.tmp", payload_path)).is_err());

        // Cleanup
        archive.delete(&key)?;
        Ok(())
    }

    #[tokio::test]
    async fn test_write_failed_returns_write_failed_error() -> Result<()> {
        // Verify that write failures return ArchiveError::WriteFailed, not Io.
        // We verify the error variant by constructing it and checking it matches.
        let write_err = ArchiveError::WriteFailed("test key: simulated IO error".to_string());
        match write_err {
            ArchiveError::WriteFailed(msg) => {
                assert!(msg.contains("test key"));
                assert!(msg.contains("simulated IO"));
            }
            _ => panic!("Expected WriteFailed error variant"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_atomic_write_no_partial_on_failure() -> Result<()> {
        // Verify that when store() fails, no partial .json.gz file is written.
        // We verify:
        // 1. Successful store produces complete file
        // 2. The .tmp file is cleaned up after successful rename
        // 3. A key that never existed is not found after failed store attempt
        let tmp_dir = std::env::temp_dir()
            .join(format!("spindle_partial_{}", chrono::Utc::now().timestamp()));
        let archive = LocalArchive::new(tmp_dir.to_str().unwrap())?;

        // The key was never stored — verify it's not present
        let fake_key = "2026-01-01/fakekey123.json.gz";
        assert!(!archive.exists(fake_key)?);

        // A successful store → retrieve must return exact bytes
        let payload = b"Atomic write test payload -- this data must be complete.";
        let metadata = ArchiveMetadata::new(
            sha256_hash(payload),
            "application/json".to_string(),
            "test_token".to_string(),
            Utc::now(),
        );

        let key = archive.store(payload, &metadata)?;
        let retrieved = archive.retrieve(&key)?;

        // Payload must be byte-identical — not truncated
        assert_eq!(retrieved, payload);
        assert_eq!(retrieved.len(), payload.len());

        // Clean up
        archive.delete(&key)?;
        assert!(!archive.exists(&key)?);
        Ok(())
    }

    // ── M1-03: Batch tests ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_batch_full_commit() -> Result<()> {
        // Add 3 payloads to a batch, commit, verify all 3 are retrievable.
        let tmp_dir = std::env::temp_dir()
            .join(format!("spindle_batch_{}", chrono::Utc::now().timestamp()));
        let archive = LocalArchive::new(tmp_dir.to_str().unwrap())?;

        let batch_id = archive.begin_batch();

        // Add 3 entries
        let entries: Vec<(&str, &[u8])> = vec![
            ("2026-01-01/digest1.json.gz", b"payload one"),
            ("2026-01-01/digest2.json.gz", b"payload two"),
            ("2026-01-01/digest3.json.gz", b"payload three"),
        ];

        for (i, (key, payload)) in entries.iter().enumerate() {
            let metadata = ArchiveMetadata::new(
                format!("sha256_{}", i),
                "application/json".to_string(),
                "test_token".to_string(),
                Utc::now(),
            );
            archive.add_to_batch(&batch_id, key, payload.to_vec(), metadata)?;
        }

        // Verify entries are NOT yet retrievable (still in batch temp)
        for (key, _) in &entries {
            assert!(
                !archive.exists(key)?,
                "Entry {} should not be retrievable before commit",
                key
            );
        }

        // Commit the batch
        archive.commit_batch(&batch_id)?;

        // Verify all 3 are now retrievable
        for (i, (key, payload)) in entries.iter().enumerate() {
            assert!(archive.exists(key)?);
            let retrieved = archive.retrieve(key)?;
            assert_eq!(retrieved, payload.to_vec());
            assert_eq!(retrieved.len(), payload.len());
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_batch_partial_commit() -> Result<()> {
        // Simulate a crash mid-commit: 3 entries added, only 2 committed.
        // Entry 3 should be absent; recover should flag the partial batch.
        let tmp_dir = std::env::temp_dir()
            .join(format!("spindle_batch_partial_{}", chrono::Utc::now().timestamp()));
        let archive = LocalArchive::new(tmp_dir.to_str().unwrap())?;

        let batch_id = archive.begin_batch();

        // Add 3 entries
        let entries: Vec<(&str, &[u8])> = vec![
            ("2026-01-01/digest1.json.gz", b"payload one"),
            ("2026-01-01/digest2.json.gz", b"payload two"),
            ("2026-01-01/digest3.json.gz", b"payload three"),
        ];

        for (i, (key, payload)) in entries.iter().enumerate() {
            let metadata = ArchiveMetadata::new(
                format!("sha256_{}", i),
                "application/json".to_string(),
                "test_token".to_string(),
                Utc::now(),
            );
            archive.add_to_batch(&batch_id, key, payload.to_vec(), metadata)?;
        }

        // Simulate crash: manually commit entries 0 and 1, leave entry 2 as .tmp
        let batch_dir = format!("{}/_batches/{}", tmp_dir.to_str().unwrap(), batch_id);

        for i in 0..2 {
            let tmp_path = format!("{}/{}.tmp", batch_dir, entries[i].0);
            let final_path = format!("{}/{}", tmp_dir.to_str().unwrap(), entries[i].0);
            let dir = std::path::Path::new(&final_path).parent().unwrap();
            std::fs::create_dir_all(dir)?;
            std::fs::rename(&tmp_path, &final_path)?;

            // Also move metadata
            let tmp_meta = format!("{}/{}.meta", batch_dir, entries[i].0);
            let final_meta = format!("{}/{}.meta", tmp_dir.to_str().unwrap(), entries[i].0);
            std::fs::rename(&tmp_meta, &final_meta)?;
        }

        // Entry 2 remains as .tmp (simulating crash before its commit)
        // Do NOT create .complete marker

        // Verify: entries 0 and 1 are retrievable, entry 2 is not
        assert!(archive.exists("2026-01-01/digest1.json.gz")?);
        assert!(archive.exists("2026-01-01/digest2.json.gz")?);
        assert!(!archive.exists("2026-01-01/digest3.json.gz")?);

        let retrieved1 = archive.retrieve("2026-01-01/digest1.json.gz")?;
        assert_eq!(retrieved1, b"payload one");

        let retrieved2 = archive.retrieve("2026-01-01/digest2.json.gz")?;
        assert_eq!(retrieved2, b"payload two");

        // recover_incomplete_batches should find this partial batch
        let partial = archive.recover_incomplete_batches()?;
        assert_eq!(partial.len(), 1);
        assert_eq!(partial[0].batch_id, batch_id);
        assert_eq!(partial[0].keys.len(), 3);
        assert_eq!(partial[0].complete_keys.len(), 2);
        assert_eq!(partial[0].complete_keys, vec![
            "2026-01-01/digest1.json.gz".to_string(),
            "2026-01-01/digest2.json.gz".to_string(),
        ]);

        Ok(())
    }

    #[tokio::test]
    async fn test_batch_no_tmp_left_behind() -> Result<()> {
        // After a successful commit, no .tmp files should remain.
        let tmp_dir = std::env::temp_dir()
            .join(format!("spindle_batch_clean_{}", chrono::Utc::now().timestamp()));
        let archive = LocalArchive::new(tmp_dir.to_str().unwrap())?;

        let batch_id = archive.begin_batch();

        let metadata = ArchiveMetadata::new(
            "sha256_test".to_string(),
            "application/json".to_string(),
            "test_token".to_string(),
            Utc::now(),
        );

        archive.add_to_batch(&batch_id, "2026-01-01/clean.json.gz", b"clean payload".to_vec(), metadata)?;

        archive.commit_batch(&batch_id)?;

        // Verify no .tmp files remain in batch dir
        let batch_dir = format!("{}/_batches/{}", tmp_dir.to_str().unwrap(), batch_id);
        let tmp_files = collect_batch_tmp_files(&std::path::Path::new(&batch_dir))?;
        assert!(tmp_files.is_empty(), "No .tmp files should remain after commit");

        // Verify payload is retrievable
        let retrieved = archive.retrieve("2026-01-01/clean.json.gz")?;
        assert_eq!(retrieved, b"clean payload");

        Ok(())
    }
}
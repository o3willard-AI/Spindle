//! spindle-store: Typed store interfaces for payloads.
//!
//! Per PLANS.md M1-08/10:
//! - Store trait: store, retrieve, exists, delete, list
//! - Typed payloads: Text, Binary, Structured, JSON, Image, Audio, Video
//! - Metadata: receipt timestamp, source token, content type, payload size, payload hash

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info, warn};

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum StoreError {
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
    #[error("Payload type mismatch: expected {expected}, got {actual}")]
    TypeMismatch { expected: String, actual: String },
    #[error("Invalid payload: {0}")]
    InvalidPayload(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

// ── Metadata ────────────────────────────────────────────────────────────────

pub mod payload_metadata;

use payload_metadata::PayloadMetadata;

// ── Store trait ─────────────────────────────────────────────────────────────

/// The typed store interface. Every payload goes through this.
pub trait Store: Send + Sync + Debug {
    /// Store a payload with metadata. Returns a reference key for later retrieval.
    fn store(
        &self,
        payload: &[u8],
        metadata: &PayloadMetadata,
        content_type: &str,
    ) -> Result<String>;

    /// Retrieve payload by key. Returns raw bytes.
    fn retrieve(&self, key: &str) -> Result<Vec<u8>>;

    /// Check if a key exists.
    fn exists(&self, key: &str) -> Result<bool>;

    /// Delete a key.
    fn delete(&self, key: &str) -> Result<()>;

    /// List keys in a time range.
    fn list(&self, time_range: Option<std::ops::Range<DateTime<chrono::Utc>>>) -> Result<Vec<String>>;

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

// ── Typed Payloads ──────────────────────────────────────────────────────────

/// Typed text payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextPayload {
    pub text: String,
    pub encoding: String,
}

impl TextPayload {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            encoding: "utf-8".to_string(),
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(self.text.clone().into_bytes())
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        String::from_utf8(data.to_vec())
            .map(|text| Self {
                text,
                encoding: "utf-8".to_string(),
            })
            .map_err(|e| StoreError::InvalidPayload(format!("Failed to decode text: {}", e)))
    }
}

/// Typed binary payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryPayload {
    pub data: Vec<u8>,
    pub format: String,
}

impl BinaryPayload {
    pub fn new(data: Vec<u8>, format: &str) -> Self {
        Self {
            data,
            format: format.to_string(),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.data.clone()
    }
}

/// Typed structured payload (JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredPayload {
    pub data: serde_json::Value,
    pub schema: String,
}

impl StructuredPayload {
    pub fn new(data: serde_json::Value, schema: &str) -> Self {
        Self {
            data,
            schema: schema.to_string(),
        }
    }

    pub fn from_json(data: &str) -> Result<Self> {
        let value: serde_json::Value = serde_json::from_str(data)
            .map_err(|e| StoreError::Serialization(format!("Failed to parse JSON: {}", e)))?;
        Ok(Self {
            data: value,
            schema: "json".to_string(),
        })
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.data)
            .map_err(|e| StoreError::Serialization(format!("Failed to serialize JSON: {}", e)))
    }
}

/// Typed image payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagePayload {
    pub data: Vec<u8>,
    pub format: String,
    pub width: u32,
    pub height: u32,
}

impl ImagePayload {
    pub fn new(data: Vec<u8>, format: &str, width: u32, height: u32) -> Self {
        Self {
            data,
            format: format.to_string(),
            width,
            height,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.data.clone()
    }
}

/// Typed audio payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioPayload {
    pub data: Vec<u8>,
    pub format: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub bit_depth: u16,
}

impl AudioPayload {
    pub fn new(data: Vec<u8>, format: &str, sample_rate: u32, channels: u16, bit_depth: u16) -> Self {
        Self {
            data,
            format: format.to_string(),
            sample_rate,
            channels,
            bit_depth,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.data.clone()
    }
}

/// Typed video payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoPayload {
    pub data: Vec<u8>,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration: f64,
}

impl VideoPayload {
    pub fn new(data: Vec<u8>, format: &str, width: u32, height: u32, fps: u32, duration: f64) -> Self {
        Self {
            data,
            format: format.to_string(),
            width,
            height,
            fps,
            duration,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.data.clone()
    }
}

// ── Local Store Backend ─────────────────────────────────────────────────────

/// Local filesystem store backend.
#[derive(Debug)]
pub struct LocalStore {
    root: String,
    storage: Arc<dyn Storage>,
}

impl LocalStore {
    /// Create a new local store. `root` is the directory where data is stored.
    pub fn new(root: &str) -> Result<Self> {
        std::fs::create_dir_all(root).map_err(|e| StoreError::Io(e))?;
        info!(root = %root, "LocalStore created");

        let storage = Arc::new(LocalStorageAdapter {
            root: root.to_string(),
        });

        Ok(LocalStore {
            root: root.to_string(),
            storage,
        })
    }
}

impl Store for LocalStore {
    fn store(
        &self,
        payload: &[u8],
        metadata: &PayloadMetadata,
        content_type: &str,
    ) -> Result<String> {
        let key = build_key(&metadata.date_str(), &metadata.payload_sha256)?;

        // Write payload
        let payload_path = format!("{}/{}", self.root, key);
        let payload_dir = std::path::Path::new(&payload_path).parent().unwrap();
        std::fs::create_dir_all(payload_dir).map_err(|e| StoreError::Io(e))?;
        std::fs::write(&payload_path, payload).map_err(|e| StoreError::Io(e))?;

        // Write metadata
        let meta_path = format!("{}/{}", self.root, format!("{}.meta", key));
        let meta_bytes = serde_json::to_vec(&metadata)
            .map_err(|e| StoreError::Serialization(e.to_string()))?;
        std::fs::write(&meta_path, &meta_bytes).map_err(|e| StoreError::Io(e))?;

        info!(key = %key, "Payload stored locally");
        Ok(key)
    }

    fn retrieve(&self, key: &str) -> Result<Vec<u8>> {
        let payload_path = format!("{}/{}", self.root, key);
        match std::fs::read(&payload_path) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(StoreError::NotFound(key.to_string()))
            }
            Err(e) => Err(StoreError::Io(e)),
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

    fn list(&self, time_range: Option<std::ops::Range<DateTime<chrono::Utc>>>) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let entries: Vec<_> = std::fs::read_dir(&self.root).map_err(|e| StoreError::Io(e))?
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
        }

        Ok(keys)
    }

    fn storage(&self) -> Arc<dyn Storage> {
        self.storage.clone()
    }
}

/// Local storage adapter implementing the Storage trait.
#[derive(Debug)]
pub struct LocalStorageAdapter {
    root: String,
}

impl Storage for LocalStorageAdapter {
    fn put(&self, key: &str, data: &[u8]) -> Result<()> {
        let path = format!("{}/{}", self.root, key);
        let dir = std::path::Path::new(&path).parent().unwrap();
        std::fs::create_dir_all(dir).map_err(|e| StoreError::Io(e))?;
        std::fs::write(&path, data).map_err(|e| StoreError::Io(e))
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let path = format!("{}/{}", self.root, key);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    fn delete(&self, key: &str) -> Result<bool> {
        let path = format!("{}/{}", self.root, key);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    fn exists(&self, key: &str) -> Result<bool> {
        let path = format!("{}/{}", self.root, key);
        Ok(std::fs::metadata(&path).is_ok())
    }

    fn list_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let prefix_path = format!("{}/{}", self.root, prefix);
        let entries: Vec<_> = std::fs::read_dir(&prefix_path).map_err(|e| StoreError::Io(e))?
            .filter_map(|e| e.ok())
            .collect();

        for entry in entries {
            let path = entry.path();
            let file_name = path.file_name().unwrap().to_string_lossy().to_string();

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
            let metadata = std::fs::metadata(dir).map_err(|e| StoreError::Io(e))?;
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

    #[test]
    fn test_text_payload() {
        let payload = TextPayload::new("Hello, World!");
        assert_eq!(payload.text, "Hello, World!");
        assert_eq!(payload.encoding, "utf-8");

        let bytes = payload.to_bytes().unwrap();
        assert_eq!(bytes, b"Hello, World!");

        let decoded = TextPayload::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.text, "Hello, World!");
    }

    #[test]
    fn test_binary_payload() {
        let data = vec![1, 2, 3, 4, 5];
        let payload = BinaryPayload::new(data.clone(), "raw");
        assert_eq!(payload.data, data);
        assert_eq!(payload.format, "raw");

        let bytes = payload.to_bytes();
        assert_eq!(bytes, data);
    }

    #[test]
    fn test_json_payload() {
        let json = r#"{"key": "value", "number": 42}"#;
        let payload = StructuredPayload::from_json(json).unwrap();
        assert_eq!(payload.schema, "json");

        let json_str = payload.to_json().unwrap();
        let decoded = StructuredPayload::from_json(&json_str).unwrap();
        assert_eq!(decoded.data, payload.data);
    }

    #[test]
    fn test_local_store_basic() {
        let tmp_dir = std::env::temp_dir().join(format!("spindle_store_test_{}", chrono::Utc::now().timestamp()));
        let store = LocalStore::new(tmp_dir.to_str().unwrap()).unwrap();

        let metadata = PayloadMetadata::new(
            Utc::now(),
            "test_token".to_string(),
            "application/json".to_string(),
            b"test payload",
        );

        let key = store.store(b"test payload", &metadata, "application/json").unwrap();
        assert!(store.exists(&key).unwrap());

        let retrieved = store.retrieve(&key).unwrap();
        assert_eq!(retrieved, b"test payload");

        store.delete(&key).unwrap();
        assert!(!store.exists(&key).unwrap());
    }
}

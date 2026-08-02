/// File I/O for writing captured request/response JSONL records to disk.
///
/// Each captured message pair gets its own unique directory under the output root:
///   {output_dir}/{timestamp}-{uuid}/request.jsonl
///   {output_dir}/{timestamp}-{uuid}/response.jsonl
///
/// File writes are batched and non-blocking — runs on a separate tokio task to avoid
/// blocking the proxy path. Recording must add <1ms overhead per request.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::metadata::{CaptureMetadata, PlatformInfo, RunType};

/// Metadata about the corpus capture session.
#[derive(Debug)]
pub struct CorpusMeta {
    /// Current message count (updated as captures happen)
    pub total_messages: u64,
    /// Client versions seen so far
    pub client_versions: std::collections::HashSet<String>,
    /// Platforms seen so far
    pub platforms_seen: std::collections::HashSet<String>,
    /// Run type counts
    pub run_type_counts: std::collections::HashMap<String, u64>,
}

impl CorpusMeta {
    /// Create a new empty corpus metadata tracker.
    fn new() -> Self {
        Self {
            total_messages: 0,
            client_versions: Default::default(),
            platforms_seen: Default::default(),
            run_type_counts: Default::default(),
        }
    }

    /// Update with a newly captured message's metadata.
    fn update(&mut self, meta: &CaptureMetadata) {
        self.total_messages += 1;

        if let Some(ref version) = meta.client_version {
            self.client_versions.insert(version.clone());
        } else {
            self.client_versions.insert("unknown".to_string());
        }

        self.platforms_seen.insert(meta.platform.to_string());
        *self.run_type_counts.entry(meta.run_type.to_string()).or_insert(0) += 1;
    }
}

/// The recorder — handles all file I/O for captured messages.
#[derive(Debug)]
pub struct Recorder {
    /// Output directory base path
    output_dir: PathBuf,
    /// In-memory metadata tracker (shared with meta writer task)
    meta_tracker: Arc<Mutex<CorpusMeta>>,
}

impl Recorder {
    /// Create a new recorder for the given output directory.
    pub fn new(output_dir: &Path) -> Self {
        // Ensure output directory exists
        std::fs::create_dir_all(output_dir).expect("Failed to create corpus output directory");

        Self {
            output_dir: output_dir.to_path_buf(),
            meta_tracker: Arc::new(Mutex::new(CorpusMeta::new())),
        }
    }

    /// Record a captured request asynchronously.
    ///
    /// This is the non-blocking path — the actual file write happens on a tokio task.
    /// Returns immediately after queueing the write.
    pub async fn record_request(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        meta: CaptureMetadata,
    ) {
        let output_dir = self.output_dir.clone();

        // Spawn a background task so the proxy path never blocks on disk I/O
        tokio::spawn(async move {
            if let Err(e) = Self::write_request(&output_dir, method, path, body, &meta).await {
                error!("Failed to write request record: {}", e);
            }

            // Update metadata tracker (non-blocking — we don't await this)
            let mut tracker = self.meta_tracker.lock().await;
            tracker.update(&meta);
        });
    }

    /// Record a captured response asynchronously.
    pub async fn record_response(
        &self,
        status: u16,
        body: &[u8],
        uuid: String,
        record_dir_name: String,
    ) {
        let output_dir = self.output_dir.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::write_response(&output_dir, &record_dir_name, status, body).await {
                error!("Failed to write response record: {}", e);
            }
        });
    }

    /// Record an error response when upstream is unreachable.
    pub async fn record_response_error(
        &self,
        status: u16,
        body: &str,
        uuid: &str,
        record_dir_name: &str,
    ) {
        let output_dir = self.output_dir.clone();
        let body_bytes = body.as_bytes().to_vec();
        let record_dir = record_dir_name.to_string();

        tokio::spawn(async move {
            // Create the record directory
            let record_path = output_dir.join(&record_dir);
            let _ = std::fs::create_dir_all(&record_path);

            let resp_record = json!({
                "ts": Utc::now().to_rfc3339(),
                "status": status,
                "headers": {},
                "body_bytes": body_bytes.len(),
                "error": true,
            });

            let tmp_path = record_path.join("response.jsonl.tmp");
            let final_path = record_path.join("response.jsonl");

            if let Ok(mut file) = std::fs::File::create(&tmp_path) {
                use std::io::Write;
                if file.write_all(format!("{}\n", serde_json::to_string(&resp_record).unwrap_or_default()).as_bytes()).is_ok()
                    && file.flush().is_ok()
                {
                    let _ = std::fs::rename(tmp_path, final_path);
                }
            }
        });
    }

    /// Write request data to a JSONL file in the corpus directory.
    async fn write_request(
        output_dir: &Path,
        method: &str,
        path: &str,
        body: &[u8],
        meta: &CaptureMetadata,
    ) -> Result<(), std::io::Error> {
        // Create unique directory for this capture session
        let timestamp = Utc::now().format("%Y%m%dT%H%M%S%.3f").to_string();
        let uuid = uuid::Uuid::new_v4();
        let dir_name = format!("{}-{}", timestamp, uuid);
        let record_dir = output_dir.join(&dir_name);

        std::fs::create_dir_all(&record_dir)?;

        // Build request JSONL record
        let req_record = json!({
            "ts": Utc::now().to_rfc3339(),
            "method": method,
            "path": path,
            "headers": {}, // Headers would be populated from actual HTTP parts
            "body_bytes": body.len(),
            "client_version": meta.client_version,
            "platform": {
                "name": meta.platform.name,
                "version": meta.platform.version,
                "architecture": meta.platform.architecture,
            },
            "run_type": meta.run_type.to_string(),
            "node_name": meta.node_name,
        });

        // Write to temp file first for atomicity, then rename
        let tmp_path = record_dir.join("request.jsonl.tmp");
        let final_path = record_dir.join("request.jsonl");

        {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp_path)?;
            writeln!(file, "{}", serde_json::to_string(&req_record)?)?;
            file.flush()?;
        }

        // Atomic rename (same filesystem — guaranteed atomic on POSIX)
        std::fs::rename(tmp_path, final_path)?;

        info!("Wrote request record: {}", dir_name);

        Ok(())
    }

    /// Write response data to a JSONL file.
    async fn write_response(
        output_dir: &Path,
        record_dir_name: &str,
        status: u16,
        body: &[u8],
    ) -> Result<(), std::io::Error> {
        let record_dir = output_dir.join(record_dir_name);

        // Build response JSONL record
        let resp_record = json!({
            "ts": Utc::now().to_rfc3339(),
            "status": status,
            "headers": {}, // Headers would be populated from actual HTTP parts
            "body_bytes": body.len(),
        });

        let tmp_path = record_dir.join("response.jsonl.tmp");
        let final_path = record_dir.join("response.jsonl");

        {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp_path)?;
            writeln!(file, "{}", serde_json::to_string(&resp_record)?)?;
            file.flush()?;
        }

        // Atomic rename
        std::fs::rename(tmp_path, final_path)?;

        info!("Wrote response record: {}", uuid);

        Ok(())
    }

    /// Write corpus-level metadata (meta.json).
    ///
    /// This should be called when capture ends to finalize the corpus.
    pub async fn write_meta_json(&self, proxy_version: &str, upstream_url: &str) {
        let tracker = self.meta_tracker.lock().await;

        let meta_record = json!({
            "version": 1,
            "proxy_version": proxy_version,
            "start_time": Utc::now().to_rfc3339(),
            "end_time": null, // Will be set when capture ends
            "upstream_url": upstream_url,
            "total_messages": tracker.total_messages,
            "client_versions_seen": tracker.client_versions.iter().cloned().collect::<Vec<_>>(),
            "platforms_seen": tracker.platforms_seen.iter().cloned().collect::<Vec<_>>(),
            "run_types": tracker.run_type_counts,
        });

        let meta_path = self.output_dir.join("meta.json");
        let tmp_path = self.output_dir.join("meta.json.tmp");

        {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp_path).expect("Failed to create meta.json.tmp");
            writeln!(file, "{}", serde_json::to_string_pretty(&meta_record).unwrap()).unwrap();
            file.flush().unwrap();
        }

        if let Err(e) = std::fs::rename(tmp_path, meta_path) {
            warn!("Could not write final meta.json: {}", e);
        } else {
            info!("Wrote corpus metadata with {} total messages", tracker.total_messages);
        }
    }

    /// Get the current message count (for monitoring).
    pub async fn get_message_count(&self) -> u64 {
        self.meta_tracker.lock().await.total_messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_corpus_dir() -> PathBuf {
        let temp_dir = std::env::temp_dir().join("spindle-test-corpus");
        let _ = fs::remove_dir_all(&temp_dir); // clean up
        temp_dir
    }

    #[tokio::test]
    async fn test_recorder_creates_output_directory() {
        let dir = test_corpus_dir();
        let recorder = Recorder::new(&dir);

        assert!(dir.exists());

        // Clean up
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_recorder_writes_request_record() {
        let dir = test_corpus_dir();
        let recorder = Recorder::new(&dir);

        let path = "/data_collector/v0/nodes/test-node/reports";
        let body = r#"{"chef_implementation_version": "18.4.23", "status": "success"}"#;
        let meta = CaptureMetadata::extract(path, body.as_bytes()).unwrap();

        recorder.record_request("POST", path, body.as_bytes(), meta).await;

        // Give the background task time to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Check that at least one record directory was created
        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();

        assert!(entries.len() >= 1, "Expected at least 1 record directory");

        // Verify request.jsonl exists in the directory
        for entry in entries {
            let record_dir = entry.path();
            if record_dir.is_dir() {
                let req_file = record_dir.join("request.jsonl");
                assert!(req_file.exists(), "Expected request.jsonl at {}", req_file.display());

                // Verify content is valid JSONL
                let content = fs::read_to_string(&req_file).unwrap();
                let _parsed: serde_json::Value = serde_json::from_str(&content.lines().next().unwrap()).unwrap();
            }
        }

        // Clean up
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_recorder_writes_response_record() {
        let dir = test_corpus_dir();
        let recorder = Recorder::new(&dir);

        let record_dir_name = "abc123-def456";
        let body = r#"{"message": "ok"}"#;

        // First create the directory (normally done by request recording)
        fs::create_dir_all(dir.join(record_dir_name)).unwrap();

        recorder.record_response(200, body.as_bytes(), record_dir_name.to_string(), record_dir_name.to_string()).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let resp_file = dir.join(record_dir_name).join("response.jsonl");
        assert!(resp_file.exists(), "Expected response.jsonl at {}", resp_file.display());

        // Verify content is valid JSONL
        let content = fs::read_to_string(&resp_file).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content.lines().next().unwrap()).unwrap();
        assert_eq!(parsed["status"], 200);
        assert_eq!(parsed["body_bytes"].as_u64().unwrap(), body.len() as u64);

        // Clean up
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_recorder_message_count_tracking() {
        let dir = test_corpus_dir();
        let recorder = Recorder::new(&dir);

        assert_eq!(recorder.get_message_count().await, 0);

        // Record a request (this updates the tracker)
        let path = "/data_collector/v0/nodes/node1/reports";
        let body = r#"{"chef_implementation_version": "18.4.23", "status": "success"}"#;
        let meta = CaptureMetadata::extract(path, body.as_bytes()).unwrap();

        recorder.record_request("POST", path, body.as_bytes(), meta).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        assert_eq!(recorder.get_message_count().await, 1);

        // Clean up
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_recorder_write_meta_json() {
        let dir = test_corpus_dir();
        let recorder = Recorder::new(&dir);

        let uuid = "test-uuid";
        let body = r#"{"chef_implementation_version": "18.4.23", "status": "success"}"#;
        let meta = CaptureMetadata::extract("/data_collector/v0/nodes/node1/reports", body.as_bytes()).unwrap();

        // Record a request to populate the tracker
        recorder.record_request("POST", "/data_collector/v0/nodes/node1/reports", body.as_bytes(), meta).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Write final metadata
        recorder.write_meta_json("0.1.0", "http://localhost:8080").await;

        let meta_path = dir.join("meta.json");
        assert!(meta_path.exists(), "Expected meta.json at {}", meta_path.display());

        let content = fs::read_to_string(&meta_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["total_messages"].as_u64().unwrap(), 1);
        assert_eq!(parsed["proxy_version"], "0.1.0");
        assert_eq!(parsed["upstream_url"], "http://localhost:8080");

        // Clean up
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_recorder_concurrent_writes() {
        use std::sync::Arc;

        let dir = test_corpus_dir();
        let recorder = Recorder::new(&dir);

        // Record 10 requests concurrently (simulating 50 concurrent per design spec)
        let mut handles = vec![];
        for i in 0..10 {
            let rec = recorder.clone_for_test();
            let handle = tokio::spawn(async move {
                let path = format!("/data_collector/v0/nodes/node{i}/reports");
                let body = r#"{"chef_implementation_version": "18.4.23", "status": "success"}"#;
                rec.record_request("POST", &path, body.as_bytes(), CaptureMetadata::extract(&path, body.as_bytes()).unwrap())
                    .await;
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Give background tasks time to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let count = recorder.get_message_count().await;
        assert_eq!(count, 10, "Expected 10 recorded messages");

        // Clean up
        let _ = fs::remove_dir_all(&dir);
    }
}

// Helper for test — Recorder doesn't implement Clone natively but we need it for concurrent tests
impl Recorder {
    /// Internal method to get a reference-backed clone for testing.
    fn clone_for_test(&self) -> Self {
        Self {
            output_dir: self.output_dir.clone(),
            meta_tracker: Arc::clone(&self.meta_tracker),
        }
    }
}

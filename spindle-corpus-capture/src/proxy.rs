/// Core reverse proxy logic.
///
/// Forwards every incoming request to the upstream Automate instance while
/// recording request/response pairs and extracting metadata for corpus generation.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use reqwest::Client;
use tracing::{error, info, instrument};

use crate::config::Config;
use crate::metadata::{CaptureMetadata, MetadataError};
use crate::recorder::Recorder;

/// The reverse proxy — wraps an HTTP client and recorder.
#[derive(Debug)]
pub struct Proxy {
    /// reqwest client for upstream requests
    client: Client,
    /// Recorder for writing corpus files
    recorder: Arc<Recorder>,
    /// Upstream URL (e.g., `https://automate.example.com`)
    upstream_url: String,
    /// Maximum payload size in bytes
    max_payload: u64,
}

impl Proxy {
    /// Create a new proxy instance.
    pub fn new(config: &Config, recorder: Arc<Recorder>) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            recorder,
            upstream_url: config.get_upstream().to_string(),
            max_payload: config.max_payload,
        }
    }

    /// Handle an incoming request — record it, forward it, return the response.
    #[instrument(name = "proxy", skip_all, fields(upstream = %self.upstream_url))]
    pub async fn handle(&self, req: Request<Body>) -> Result<Response<Body>, ProxyError> {
        // 1. Extract headers and body
        let (parts, body) = req.into_parts();
        let headers = parts.headers;
        let method = parts.method.to_string();
        let path = parts.uri.path().to_string();

        // 2. Limit payload size (ING-11)
        let content_length = headers
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);

        if content_length > self.max_payload {
            info!(
                "Payload too large: {} bytes (limit: {})",
                content_length, self.max_payload
            );
            let resp = Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"error":"payload too large"}"#))
                .map_err(|e| ProxyError::ResponseBuild(e.to_string()))?;
            return Ok(resp);
        }

        // 3. Collect the body bytes (buffer it for recording + forwarding)
        let request_bytes = axum::body::to_bytes(body, self.max_payload as usize + 1)
            .await
            .map_err(|e| ProxyError::BodyRead(e.to_string()))?;

        if request_bytes.len() > self.max_payload as usize {
            info!(
                "Payload too large (decoded): {} bytes",
                request_bytes.len()
            );
            let resp = Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"error":"payload too large"}"#))
                .map_err(|e| ProxyError::ResponseBuild(e.to_string()))?;
            return Ok(resp);
        }

        // 4. Extract metadata from request path + body
        let meta_result = CaptureMetadata::extract(&path, &request_bytes);

        // 5. Generate a unique request ID for this capture
        let req_uuid = uuid::Uuid::new_v4();
        let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3f").to_string();
        let record_dir_name = format!("{}-{}", timestamp, req_uuid);

        // 6. Clone data for the forwarding task
        let upstream_url = self.upstream_url.clone();
        let recorder_clone = Arc::clone(&self.recorder);

        // 7. Forward request to upstream
        let forward_result = self.forward_request(
            &upstream_url,
            &path,
            &headers,
            &method,
            &request_bytes,
            &meta_result,
            &recorder_clone,
            &req_uuid,
            &record_dir_name,
        ).await;

        let (status, resp_bytes) = match forward_result {
            Ok(result) => result,
            Err(e) => {
                error!("Upstream request failed: {}", e);
                
                // Record the failure
                let meta = meta_result.unwrap_or_else(|_| CaptureMetadata::default_unknown());
                let body_vec = request_bytes.as_ref().to_vec();
                recorder_clone.record_request(method.clone(), path.clone(), body_vec, meta).await;
                let error_body = format!("Upstream connection failed: {}", e);
                recorder_clone.record_response_error(502, &error_body, &req_uuid.to_string(), &record_dir_name).await;
                
                let resp = Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(Body::from(error_body))
                    .map_err(|err| ProxyError::ResponseBuild(err.to_string()))?;
                return Ok(resp);
            }
        };

        // 8. Build and return the response to the client
        let resp = Response::builder()
            .status(status)
            .body(Body::from(resp_bytes))
            .map_err(|e| ProxyError::ResponseBuild(e.to_string()))?;

        Ok(resp)
    }

    /// Forward the request to upstream and record request/response pair.
    async fn forward_request(
        &self,
        upstream_url: &str,
        path: &str,
        headers: &HeaderMap,
        method: &str,
        body: &[u8],
        meta_result: &Result<CaptureMetadata, MetadataError>,
        recorder: &Arc<Recorder>,
        req_uuid: &uuid::Uuid,
        record_dir_name: &str,
    ) -> Result<(u16, Vec<u8>), ProxyError> {
        let url = format!("{}{}", upstream_url, path);

        // Build headers for forwarding — skip hop-by-hop headers
        let mut forward_headers = reqwest::header::HeaderMap::new();
        for (key, value) in headers.iter() {
            let skip = matches!(key.as_str(), "host" | "connection" | "transfer-encoding" | "keep-alive" | "proxy-connection" | "te" | "trailers" | "upgrade");
            if !skip {
                if let Ok(s) = value.to_str() {
                    if let Ok(hv) = reqwest::header::HeaderValue::from_str(s) {
                        forward_headers.insert(key.as_str().parse::<reqwest::header::HeaderName>().map_err(|e| ProxyError::BodyRead(e.to_string()))?, hv);
                    }
                }
            }
        }

        // Set Content-Length from the body we buffered
        forward_headers.insert(
            reqwest::header::CONTENT_LENGTH,
            reqwest::header::HeaderValue::from(body.len()),
        );

        // Build the upstream request
        let method_str = method.to_string();
        let mut request_builder = match method_str.as_str() {
            "POST" => self.client.post(&url),
            "PUT" => self.client.put(&url),
            "GET" => self.client.get(&url),
            "DELETE" => self.client.delete(&url),
            "HEAD" => self.client.head(&url),
            "PATCH" => self.client.patch(&url),
            _ => self.client.request(method.parse().unwrap_or_default(), &url),
        };

        request_builder = request_builder.headers(forward_headers);
        request_builder = request_builder.body(body.to_vec());

        // Send and capture response
        let resp = request_builder.send().await.map_err(|e| ProxyError::Upstream(e.to_string()))?;
        let status = resp.status().as_u16();
        let resp_bytes = resp.bytes().await.map_err(|e| ProxyError::BodyRead(e.to_string()))?;
        let resp_vec = resp_bytes.as_ref().to_vec();

        // Record the request
        let meta = match meta_result {
            Ok(m) => m.clone(),
            Err(_) => CaptureMetadata::default_unknown(),
        };
        recorder.record_request(method.to_string(), path.to_string(), body.to_vec(), meta).await;

        // Record the response
        recorder.record_response(status, resp_vec.clone(), req_uuid.to_string(), record_dir_name.to_string()).await;

        Ok((status, resp_vec))
    }
}

/// Errors that can occur during proxy operations.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("failed to read request body: {0}")]
    BodyRead(String),

    #[error("failed to build response: {0}")]
    ResponseBuild(String),

    #[error("upstream request failed: {0}")]
    Upstream(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_proxy_error_display() {
        let err = ProxyError::BodyRead("test error".to_string());
        assert!(err.to_string().contains("test error"));

        let err = ProxyError::Upstream("connection refused".to_string());
        assert!(err.to_string().contains("connection refused"));
    }

    #[tokio::test]
    async fn test_proxy_creation() {
        let temp_dir = std::env::temp_dir().join("spindle-test-proxy");
        std::fs::create_dir_all(&temp_dir).ok();

        let args = ["spindle-corpus-capture", "--upstream", "http://localhost:8080", "--output", temp_dir.to_str().unwrap()];
        let config = Config::try_parse_from(args).unwrap();
        let recorder = Arc::new(Recorder::new(&config.output));
        let proxy = Proxy::new(&config, recorder);

        assert_eq!(proxy.upstream_url, "http://localhost:8080");
        assert_eq!(proxy.max_payload, 32 * 1024 * 1024);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

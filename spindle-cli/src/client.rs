//! HTTP client for Spindle API.

#![allow(warnings)]
use serde_json::Value;

/// Simple API client for Spindle server.
pub struct ApiClient {
    pub base_url: String,
    token: String,
    client: reqwest::Client,
}

impl ApiClient {
    pub fn new(base_url: &str, token: &str) -> Self {
        let client = if token.is_empty() {
            reqwest::Client::new()
        } else {
            reqwest::Client::builder()
                .default_headers({
                    let mut headers = reqwest::header::HeaderMap::new();
                    headers.insert(
                        reqwest::header::AUTHORIZATION,
                        format!("Bearer {}", token)
                            .parse()
                            .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static("")),
                    );
                    headers
                })
                .build()
                .unwrap_or_default()
        };
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            client,
        }
    }

    pub async fn get_json(&self, path: &str) -> Result<Value, Box<dyn std::error::Error>> {
        let url = format!("{}/{}", self.base_url, path);
        let resp = self.client.get(&url).send().await?;
        let data: Value = resp.json().await?;
        Ok(data)
    }

    /// GET an endpoint and return the raw response body as text.
    /// Used for JSONL exports where the response is not a single JSON object.
    pub async fn get_text(&self, path: &str) -> Result<String, Box<dyn std::error::Error>> {
        let url = format!("{}/{}", self.base_url, path);
        let resp = self.client.get(&url).send().await?;
        let text = resp.text().await?;
        Ok(text)
    }

    pub async fn post_json(
        &self,
        path: &str,
        body: &Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let url = format!("{}/{}", self.base_url, path);
        let resp = self.client.post(&url).json(body).send().await?;
        if resp.status().is_success() {
            let data: Value = resp.json().await?;
            Ok(data)
        } else {
            Ok(serde_json::json!(
                {
                    "status": "error",
                    "http_status": resp.status().as_u16(),
                    "message": resp.text().await.unwrap_or_default()
            }
            ))
        }
    }

    pub async fn patch_json(
        &self,
        path: &str,
        body: &Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let url = format!("{}/{}", self.base_url, path);
        let resp = self.client.patch(&url).json(body).send().await?;
        let data: Value = resp.json().await?;
        Ok(data)
    }

    pub async fn delete(&self, path: &str) -> Result<u16, Box<dyn std::error::Error>> {
        let url = format!("{}/{}", self.base_url, path);
        let resp = self.client.delete(&url).send().await?;
        Ok(resp.status().as_u16())
    }

    /// GET an endpoint with HTTP status check.
    /// Returns (status_code, response_body_text).
    pub async fn get_with_status(
        &self,
        path: &str,
    ) -> Result<(u16, String), Box<dyn std::error::Error>> {
        let url = format!("{}/{}", self.base_url, path);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, text))
    }

    pub async fn health_check(&self) -> Result<Value, Box<dyn std::error::Error>> {
        let url = format!("{}/v1/health", self.base_url);
        let resp = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await?;
        let data: Value = resp.json().await?;
        Ok(data)
    }
}

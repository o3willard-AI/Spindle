//! Thin HTTP client for the Spindle REST API. Stateless: each call is
//! independent and authenticated with the bearer token supplied per request.

use crate::AppState;
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub enum ApiError {
    /// 401/403 — the supplied token was missing or rejected.
    Unauthorized(u16, String),
    /// Other non-success status with the raw body.
    Http(u16, String),
    /// Transport-level failure (API unreachable).
    Transport(String),
    /// Response body did not match the expected shape.
    Parse(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Unauthorized(code, body) => {
                write!(f, "unauthorized ({code}): {body}")
            }
            ApiError::Http(code, body) => write!(f, "http {code}: {body}"),
            ApiError::Transport(msg) => write!(f, "api unreachable: {msg}"),
            ApiError::Parse(msg) => write!(f, "bad response: {msg}"),
        }
    }
}

/// Extract the API token from inbound request headers.
///
/// Accepts both `X-Api-Token` and the standard `Authorization: Bearer <tok>`
/// form so the dashboard can be driven either by the login page or by a
/// straight HTTP client / reverse-proxy header.
pub fn extract_token(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("x-api-token").and_then(|v| v.to_str().ok()) {
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").filter(|b| !b.is_empty()))
        .map(|b| b.to_string())
}

/// Perform a `GET` against the API and deserialize the JSON body.
pub async fn api_get<T: DeserializeOwned>(
    state: &AppState,
    path: &str,
    token: &Option<String>,
) -> Result<T, ApiError> {
    let url = format!("{}{}", state.api_url, path);
    let mut req = state.client.get(&url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| ApiError::Transport(e.to_string()))?;
    let status_code = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();

    if status_code == 401 || status_code == 403 {
        return Err(ApiError::Unauthorized(status_code, body));
    }
    if !(200..300).contains(&status_code) {
        return Err(ApiError::Http(status_code, body));
    }

    serde_json::from_str::<T>(&body).map_err(|e| ApiError::Parse(e.to_string()))
}

/// Fetch a JSON array out of a `{ data: [...] }` envelope, tolerating
/// `{ data: { items: [...] } }` shapes (compliance).
fn extract_data_array(v: &serde_json::Value) -> Vec<serde_json::Value> {
    match v.get("data") {
        Some(serde_json::Value::Array(arr)) => arr.clone(),
        Some(serde_json::Value::Object(obj)) => obj
            .get("items")
            .and_then(|i| i.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Fetch the `data` field of a JSON envelope as a typed list.
pub async fn api_list<T: DeserializeOwned>(
    state: &AppState,
    path: &str,
    token: &Option<String>,
) -> Result<Vec<T>, ApiError> {
    let v: serde_json::Value = api_get(state, path, token).await?;
    let arr = extract_data_array(&v);
    arr.into_iter()
        .map(|item| serde_json::from_value(item).map_err(|e| ApiError::Parse(e.to_string())))
        .collect()
}

//! Embedded SPA serving + /v1 reverse proxy for the React frontend.
//!
//! The frontend (frontend/, Vite → dist/) is embedded into the binary at
//! compile time via rust-embed, so `spindle-dashboard` ships as a single
//! self-contained artifact:
//!
//! - `GET /` and any non-API GET → `index.html` (SPA fallback for deep links)
//! - `GET /assets/*` → hashed static assets from dist/assets
//! - `/v1/*` → proxied to the Spindle API with the caller's credentials
//!   (`X-Api-Token` / `Authorization: Bearer`) forwarded untouched.
//!
//! The proxy is what keeps the SPA same-origin: the frontend calls relative
//! `/v1/...` URLs and stores its token in localStorage.

#![allow(warnings)]
use axum::{
    body::Body,
    http::{Request, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use rust_embed::RustEmbed;

/// Compiled-in frontend assets (frontend/dist at build time).
#[derive(RustEmbed)]
#[folder = "../frontend/dist/"]
struct FrontendAssets;

/// Serve an embedded file by path ("" and "/" map to index.html).
fn serve_asset(path: &str) -> Response {
    let clean = path.trim_start_matches('/');
    let key = if clean.is_empty() {
        "index.html"
    } else {
        clean
    };

    match FrontendAssets::get(key) {
        Some(file) => {
            let mime = mime_guess::from_path(key).first_or_octet_stream();
            (
                [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
                file.data,
            )
                .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            "asset not found (was the frontend built? run: cd frontend && bun run build)",
        )
            .into_response(),
    }
}

/// index.html for both / and any unknown non-API path (client-side router).
pub async fn spa_index() -> Response {
    Html(
        String::from_utf8_lossy(
            &FrontendAssets::get("index.html")
                .map(|f| f.data.to_vec())
                .unwrap_or_else(|| {
                    b"<h1>spindle-dashboard</h1><p>frontend/dist missing at build time</p>".to_vec()
                }),
        )
        .to_string(),
    )
    .into_response()
}

/// GET /assets/:path — hashed build artifacts.
pub async fn asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    serve_asset(&format!("assets/{path}"))
}

/// Reverse-proxy any /v1/* request to the Spindle API.
///
/// Forwards method, path+query, body, Content-Type and the caller's auth
/// headers (X-Api-Token / Authorization) verbatim; returns the API's status
/// code, content type and body unchanged so the SPA sees exactly the same
/// envelopes it would against a direct API connection.
pub async fn proxy_v1(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    uri: Uri,
    request: Request<Body>,
) -> Response {
    let path = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/v1");
    let url = format!("{}{}", state.api_url, path);

    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, 32 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("failed to read request body: {e}"),
            )
                .into_response()
        }
    };

    let mut out = state.client.request(parts.method.clone(), &url);
    // Forward credentials + payload metadata. The API itself authenticates
    // with `Authorization: Bearer`, while browser clients may send
    // `X-Api-Token` (the legacy dashboard header) — normalize it to Bearer so
    // both forms keep working end-to-end.
    let auth = parts.headers.get("authorization").cloned().or_else(|| {
        parts.headers.get("x-api-token").and_then(|v| {
            v.to_str().ok().map(|t| {
                axum::http::HeaderValue::from_str(&format!("Bearer {t}"))
                    .unwrap_or_else(|_| axum::http::HeaderValue::from_static(""))
            })
        })
    });
    if let Some(v) = auth {
        out = out.header("authorization", v);
    }
    for name in ["content-type", "accept"] {
        if let Some(v) = parts.headers.get(name) {
            out = out.header(name, v);
        }
    }

    match out.body(bytes).send().await {
        Ok(resp) => {
            let status = resp.status();
            let ct = resp
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            match resp.bytes().await {
                Ok(bytes) => {
                    let mut response = Response::builder().status(status);
                    if let Some(ct) = ct {
                        response = response.header(axum::http::header::CONTENT_TYPE, ct);
                    }
                    response.body(Body::from(bytes)).unwrap_or_else(|e| {
                        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
                    })
                }
                Err(e) => (
                    StatusCode::BAD_GATEWAY,
                    axum::Json(serde_json::json!({"error": {"code": "upstream_read_failed", "message": e.to_string()}})),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            axum::Json(
                serde_json::json!({"error": {"code": "api_unreachable", "message": e.to_string()}}),
            ),
        )
            .into_response(),
    }
}

/// Mount SPA serving + proxy onto the dashboard router.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/", get(spa_index))
        .route("/assets/*path", get(asset))
        .fallback(get(spa_fallback))
}

async fn spa_fallback(uri: Uri) -> Response {
    // Anything that is not /v1/* and not a real asset gets index.html so
    // client-side deep links (e.g. /nodes/<id>) work on refresh.
    tracing::debug!(path = %uri.path(), "SPA fallback");
    spa_index().await
}

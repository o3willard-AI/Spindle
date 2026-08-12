use axum::middleware::Next;
use axum::response::Response;
use tower::util::ServiceExt;

use crate::request_id::generate_request_id;

/// Axum middleware that injects `X-Request-Id` into every response.
///
/// The request_id is attached to the `tracing` span and propagated
/// via the `X-Request-Id` header on the response.
pub async fn request_id_middleware(
    next: Next,
    request: axum::extract::Request,
) -> Result<Response, axum::BoxError> {
    let request_id = generate_request_id();

    // Attach request_id to the current span.
    let _enter = tracing::info_span!("request", request_id = request_id.to_string());

    let response = next
        .oneshot(request)
        .await
        .map_err(axum::BoxError::from)?;

    // Inject the request_id into the response headers.
    Response::builder()
        .header("x-request-id", request_id.to_string())
        .body(response.into_body())
        .map_err(axum::BoxError::from)
}

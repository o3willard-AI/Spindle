//! Spindle error handling — domain errors + API error responses.
//!
//! # Usage
//! ```ignore
//! use spindle_error::{Error, ApiError};
//!
//! let err = Error::Ingest("corrupt archive".into());
//! let response = err.into_response(); // returns 400 Bad Request
//! ```

use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::fmt;
use thiserror::Error;

/// Domain errors from spindle subsystems.
#[derive(Debug, Error)]
pub enum Error {
    /// Ingest pipeline error (raw archive processing).
    #[error("ingest error: {0}")]
    Ingest(String),

    /// Store error (persistent storage).
    #[error("store error: {0}")]
    Store(String),

    /// Pipeline error (data processing).
    #[error("pipeline error: {0}")]
    Pipeline(String),

    /// Validation error (input validation).
    #[error("validation error: {0}")]
    Validation(String),

    /// Not found error.
    #[error("not found: {0}")]
    NotFound(String),

    /// Internal server error.
    #[error("internal error: {0}")]
    Internal(String),

    /// Authentication error.
    #[error("authentication error: {0}")]
    Authentication(String),

    /// Authorization error.
    #[error("authorization error: {0}")]
    Authorization(String),

    /// Rate limit error.
    #[error("rate limit: {0}")]
    RateLimit(String),
}

/// API error with machine-readable code, human message, and optional details.
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub code: u16,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "api error (code {}): {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

/// Trait for types that can be converted to API errors.
pub trait ApiErrorTrait {
    /// Machine-readable HTTP status code.
    fn code(&self) -> u16;

    /// Human-readable error message.
    fn message(&self) -> &str;

    /// Optional detailed error information.
    fn details(&self) -> Option<&str> {
        None
    }

    /// Optional request ID for tracing.
    fn request_id(&self) -> Option<&str> {
        None
    }
}

impl ApiErrorTrait for Error {
    fn code(&self) -> u16 {
        match self {
            Self::Ingest(_) => 400,
            Self::Store(_) => 500,
            Self::Pipeline(_) => 500,
            Self::Validation(_) => 400,
            Self::NotFound(_) => 404,
            Self::Internal(_) => 500,
            Self::Authentication(_) => 401,
            Self::Authorization(_) => 403,
            Self::RateLimit(_) => 429,
        }
    }

    fn message(&self) -> &str {
        // Return the variant's message (the part after the prefix)
        match self {
            Self::Ingest(msg) => msg.as_str(),
            Self::Store(msg) => msg.as_str(),
            Self::Pipeline(msg) => msg.as_str(),
            Self::Validation(msg) => msg.as_str(),
            Self::NotFound(msg) => msg.as_str(),
            Self::Internal(msg) => msg.as_str(),
            Self::Authentication(msg) => msg.as_str(),
            Self::Authorization(msg) => msg.as_str(),
            Self::RateLimit(msg) => msg.as_str(),
        }
    }

    fn details(&self) -> Option<&str> {
        Some(self.message())
    }

    fn request_id(&self) -> Option<&str> {
        None
    }
}

impl axum::response::IntoResponse for Error {
    fn into_response(self) -> Response {
        let code = self.code();
        let message = self.message().to_string();
        let details = self.details().map(|s| s.to_string());
        let request_id = self.request_id().map(|s| s.to_string());

        let api_error = ApiError {
            code,
            message,
            details,
            request_id,
        };

        axum::response::Json(api_error).into_response()
    }
}

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        ApiError {
            code: err.code(),
            message: err.message().to_string(),
            details: err.details().map(|s| s.to_string()),
            request_id: err.request_id().map(|s| s.to_string()),
        }
    }
}

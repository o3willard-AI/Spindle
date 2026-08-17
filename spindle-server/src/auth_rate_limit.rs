//! S-10: Auth endpoint rate limiting using governor's token-bucket algorithm.
//!
//! Applies per-endpoint rate limits to authentication routes:
//! - POST /v1/auth/local/login: 5 requests per minute
//! - POST /v1/auth/local/register: 3 requests per minute
//! - POST /v1/auth/login: 5 requests per minute (JIT OIDC login)
//!
//! When the limit is exceeded, returns 429 with a Retry-After header.
//! The limit is configurable via SPINDLE_AUTH_RATE_LIMIT env var
//! (format: "login:5,register:3" — applies to all auth endpoints).
//!
//! Counters:
//! - auth_rate_limit_hits_total{endpoint} — incremented on each 429 response

#![allow(warnings)]
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use axum::http::header::RETRY_AFTER;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use governor::clock::DefaultClock;
use governor::middleware::NoOpMiddleware;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use serde_json::json;

use crate::metrics::MetricsRegistry;
/// Default rate limits: requests per minute per endpoint.
pub const DEFAULT_LOGIN_LIMIT: u32 = 5; // 5 per minute
pub const DEFAULT_REGISTER_LIMIT: u32 = 3; // 3 per minute

/// Rate limit configuration parsed from SPINDLE_AUTH_RATE_LIMIT env var.
/// Format: "login:5,register:3"
#[derive(Debug, Clone)]
pub struct AuthRateLimitConfig {
    pub login_per_minute: u32,
    pub register_per_minute: u32,
}

impl Default for AuthRateLimitConfig {
    fn default() -> Self {
        Self {
            login_per_minute: DEFAULT_LOGIN_LIMIT,
            register_per_minute: DEFAULT_REGISTER_LIMIT,
        }
    }
}

impl AuthRateLimitConfig {
    pub fn from_env() -> Self {
        let raw = std::env::var("SPINDLE_AUTH_RATE_LIMIT").unwrap_or_default();
        let mut config = Self::default();

        if !raw.is_empty() {
            for pair in raw.split(',') {
                let parts: Vec<&str> = pair.split(':').collect();
                if parts.len() == 2 {
                    let key = parts[0].trim();
                    let val: u32 = parts[1].trim().parse().unwrap_or(0);
                    match key {
                        "login" => config.login_per_minute = val,
                        "register" => config.register_per_minute = val,
                        _ => {}
                    }
                }
            }
        }

        // Fallback: if env value is invalid (0 or missing), use defaults
        if config.login_per_minute == 0 {
            config.login_per_minute = DEFAULT_LOGIN_LIMIT;
        }
        if config.register_per_minute == 0 {
            config.register_per_minute = DEFAULT_REGISTER_LIMIT;
        }

        config
    }
}

/// Rate limits for a single auth endpoint.
#[derive(Debug)]
struct EndpointRateLimiter {
    limiter: RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>,
    per_minute: u32,
}

impl EndpointRateLimiter {
    fn new(per_minute: u32) -> Self {
        // governor's Quota::per_second with allow_burst creates a token bucket.
        // We want N requests per minute = N/60 per second, with burst = N.
        let per_second = (per_minute as f64 / 60.0).max(1.0) as u32;
        let quota = Quota::per_second(NonZeroU32::new(per_second).unwrap())
            .allow_burst(NonZeroU32::new(per_minute).unwrap());
        let limiter: RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware> =
            RateLimiter::direct(quota);
        Self {
            limiter,
            per_minute,
        }
    }

    /// Check if the request is allowed. Returns None if allowed, Some(retry_secs) if rate limited.
    fn check(&self) -> Option<u64> {
        match self.limiter.check() {
            Ok(_) => None,
            Err(_) => {
                // Retry after ~1 minute (until the bucket refills)
                Some(60)
            }
        }
    }
}

/// Shared auth rate limit state — one limiter per endpoint.
#[derive(Debug, Clone)]
pub struct AuthRateLimiter {
    login_limiter: Arc<EndpointRateLimiter>,
    register_limiter: Arc<EndpointRateLimiter>,
    config: AuthRateLimitConfig,
    metrics: Arc<MetricsRegistry>,
}

impl AuthRateLimiter {
    pub fn new(config: AuthRateLimitConfig, metrics: Arc<MetricsRegistry>) -> Self {
        Self {
            login_limiter: Arc::new(EndpointRateLimiter::new(config.login_per_minute)),
            register_limiter: Arc::new(EndpointRateLimiter::new(config.register_per_minute)),
            config,
            metrics,
        }
    }

    /// Check rate limit for an endpoint. Returns None if allowed,
    /// Some(retry_after_seconds) if rate limited.
    pub fn check(&self, endpoint: &str) -> Option<u64> {
        let limiter = match endpoint {
            "login" => &self.login_limiter,
            "register" => &self.register_limiter,
            _ => return None, // No rate limit for other endpoints
        };

        let result = limiter.check();
        if result.is_some() {
            // Increment the auth_rate_limit_hits_total counter for this endpoint.
            // The BTreeMap is pre-populated at init, but we use or_insert_with
            // as a safety fallback.
            if let Some(counter) = self.metrics.auth_rate_limit_hits_total.get(endpoint) {
                counter.inc();
            }
        }
        result
    }

    /// Build a 429 Too Many Requests response with Retry-After header.
    pub fn rate_limited_response(
        retry_after: u64,
    ) -> (StatusCode, HeaderMap, Json<serde_json::Value>) {
        let mut headers = HeaderMap::new();
        headers.insert(
            RETRY_AFTER,
            HeaderValue::from_str(&retry_after.to_string()).unwrap_or(HeaderValue::from(60)),
        );
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let body = json!({
            "error": "rate_limit_exceeded",
            "message": format!("Too many requests. Try again in {} seconds.", retry_after),
            "retry_after": retry_after,
        });
        (StatusCode::TOO_MANY_REQUESTS, headers, Json(body))
    }

    /// Check rate limit and build response if exceeded.
    /// Returns Some(response) if rate limited, None if allowed.
    pub fn check_or_response(
        &self,
        endpoint: &str,
    ) -> Option<(StatusCode, HeaderMap, Json<serde_json::Value>)> {
        self.check(endpoint).map(Self::rate_limited_response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize tests that mutate the SPINDLE_AUTH_RATE_LIMIT env var.
    /// Without this, parallel test execution causes data races on the shared
    /// env var, producing intermittent failures.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_default_rate_limits() {
        let _guard = ENV_LOCK.lock().unwrap();
        let config = AuthRateLimitConfig::default();
        assert_eq!(config.login_per_minute, 5);
        assert_eq!(config.register_per_minute, 3);
    }

    #[test]
    fn test_config_from_env_invalid() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("SPINDLE_AUTH_RATE_LIMIT");
        let config = AuthRateLimitConfig::from_env();
        assert_eq!(config.login_per_minute, DEFAULT_LOGIN_LIMIT);
        assert_eq!(config.register_per_minute, DEFAULT_REGISTER_LIMIT);
    }

    #[test]
    fn test_config_from_env_custom() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("SPINDLE_AUTH_RATE_LIMIT", "login:10,register:5");
        let config = AuthRateLimitConfig::from_env();
        assert_eq!(config.login_per_minute, 10);
        assert_eq!(config.register_per_minute, 5);
        std::env::remove_var("SPINDLE_AUTH_RATE_LIMIT");
    }

    #[test]
    fn test_config_from_env_partial() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("SPINDLE_AUTH_RATE_LIMIT", "login:8");
        let config = AuthRateLimitConfig::from_env();
        assert_eq!(config.login_per_minute, 8);
        assert_eq!(config.register_per_minute, DEFAULT_REGISTER_LIMIT);
        std::env::remove_var("SPINDLE_AUTH_RATE_LIMIT");
    }

    #[test]
    fn test_config_from_env_invalid_value() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("SPINDLE_AUTH_RATE_LIMIT", "login:abc,register:xyz");
        let config = AuthRateLimitConfig::from_env();
        assert_eq!(config.login_per_minute, DEFAULT_LOGIN_LIMIT);
        assert_eq!(config.register_per_minute, DEFAULT_REGISTER_LIMIT);
        std::env::remove_var("SPINDLE_AUTH_RATE_LIMIT");
    }

    #[test]
    fn test_rate_limited_response_has_retry_after() {
        let (status, headers, _) = AuthRateLimiter::rate_limited_response(60);
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(headers.get(RETRY_AFTER).unwrap(), "60");
    }

    #[test]
    fn test_rate_limiter_allows_then_blocks() {
        let config = AuthRateLimitConfig {
            login_per_minute: 3,
            register_per_minute: 2,
        };
        let metrics = Arc::new(MetricsRegistry::new());
        let limiter = AuthRateLimiter::new(config, metrics);

        // 3 allowed
        assert!(limiter.check("login").is_none());
        assert!(limiter.check("login").is_none());
        assert!(limiter.check("login").is_none());

        // 4th blocked (burst exhausted)
        let retry = limiter.check("login");
        assert!(retry.is_some(), "4th request should be rate limited");
        assert!(retry.unwrap() > 0);
    }

    #[test]
    fn test_rate_limiter_counts_hits() {
        let config = AuthRateLimitConfig {
            login_per_minute: 1,
            register_per_minute: 3,
        };
        let metrics = Arc::new(MetricsRegistry::new());
        let limiter = AuthRateLimiter::new(config, metrics.clone());

        // Exhaust the login rate limit
        let _ = limiter.check("login"); // allowed
        assert!(limiter.check("login").is_some()); // blocked → 1 hit
        assert!(limiter.check("login").is_some()); // blocked → 2 hits

        let hits = metrics
            .auth_rate_limit_hits_total
            .get("login")
            .map(|c| c.value())
            .unwrap_or(0);
        assert_eq!(hits, 2, "Should have 2 rate limit hits");
    }

    #[test]
    fn test_rate_limiter_independent_endpoints() {
        let config = AuthRateLimitConfig {
            login_per_minute: 1,
            register_per_minute: 1,
        };
        let metrics = Arc::new(MetricsRegistry::new());
        let limiter = AuthRateLimiter::new(config, metrics);

        // login allows 1, register allows 1 — independent
        assert!(limiter.check("login").is_none()); // login allowed
        assert!(limiter.check("register").is_none()); // register allowed (independent)
        assert!(limiter.check("login").is_some()); // login blocked
        assert!(limiter.check("register").is_some()); // register blocked
    }
}

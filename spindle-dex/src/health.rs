//! Dex health check module.
//!
//! Polls the Dex `/.well-known/openid-configuration` endpoint to verify
//! the Dex server is running and responding correctly before Spindle
//! starts serving.

use crate::DexConfig;
use reqwest::Client;
use std::time::Duration;

/// Health check result for the Dex server.
#[derive(Debug, Clone)]
pub struct DexHealth {
    /// Whether Dex is healthy.
    pub is_healthy: bool,
    /// Response status code.
    pub status_code: u16,
    /// Last error message.
    pub error: Option<String>,
    /// Discovery document content (if available).
    pub discovery_doc: Option<String>,
}

impl std::fmt::Display for DexHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_healthy {
            write!(f, "Dex is healthy")
        } else {
            write!(
                f,
                "Dex is not healthy (status: {}, error: {:?})",
                self.status_code, self.error
            )
        }
    }
}

/// Result of a health check.
pub type HealthCheckResult = Result<DexHealth, String>;

/// Perform a single health check against the Dex server.
///
/// Makes a GET request to `issuer_url + "/.well-known/openid-configuration"`.
/// Returns a `DexHealth` struct with the result.
pub async fn check_health(config: &DexConfig) -> HealthCheckResult {
    let url = format!("{}/.well-known/openid-configuration", config.issuer_url);
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Health check request failed: {}", e))?;

    let status_code = response.status().as_u16();
    let discovery_doc = response
        .text()
        .await
        .map_err(|e| format!("Failed to read discovery document: {}", e))?;

    Ok(DexHealth {
        is_healthy: status_code == 200,
        status_code,
        error: if status_code != 200 {
            Some(format!("Health check failed with status: {}", status_code))
        } else {
            None
        },
        discovery_doc: if status_code == 200 {
            Some(discovery_doc)
        } else {
            None
        },
    })
}

/// Poll the Dex server until it's healthy or a timeout occurs.
///
/// Checks the health endpoint every `interval` for up to `max_attempts` times.
///
/// # Arguments
/// * `config` - Dex configuration with the issuer URL
/// * `interval` - Time between health checks (default: 500ms)
/// * `max_attempts` - Maximum number of attempts (default: 30, ~15 seconds)
pub async fn poll_health(
    config: &DexConfig,
    interval: Duration,
    max_attempts: u32,
) -> HealthCheckResult {
    for attempt in 0..max_attempts {
        match check_health(config).await {
            Ok(health) => {
                if health.is_healthy {
                    tracing::info!("Dex health check passed after {} attempts", attempt + 1);
                    return Ok(health);
                } else {
                    tracing::debug!(
                        "Dex health check attempt {} failed: {}",
                        attempt + 1,
                        health
                    );
                }
            }
            Err(e) => {
                tracing::debug!("Dex health check attempt {} error: {}", attempt + 1, e);
            }
        }

        if attempt < max_attempts - 1 {
            tokio::time::sleep(interval).await;
        }
    }

    Err(format!(
        "Dex health check failed after {} attempts",
        max_attempts
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_check_health_unreachable() {
        let config = DexConfig {
            issuer: "https://nonexistent.local/dex".to_string(),
            issuer_url: "https://nonexistent.local/dex".to_string(),
            health_check: true,
            connectors: vec![],
            features: crate::Features::default(),
        };

        let result = check_health(&config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_poll_health_timeout() {
        let config = DexConfig {
            issuer: "https://nonexistent.local/dex".to_string(),
            issuer_url: "https://nonexistent.local/dex".to_string(),
            health_check: true,
            connectors: vec![],
            features: crate::Features::default(),
        };

        let result = poll_health(&config, Duration::from_millis(100), 3).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed after 3 attempts"));
    }

    #[test]
    fn test_health_display_healthy() {
        let health = DexHealth {
            is_healthy: true,
            status_code: 200,
            error: None,
            discovery_doc: Some("".to_string()),
        };

        assert_eq!(health.to_string(), "Dex is healthy");
    }

    #[test]
    fn test_health_display_unhealthy() {
        let health = DexHealth {
            is_healthy: false,
            status_code: 500,
            error: Some("Internal server error".to_string()),
            discovery_doc: None,
        };

        assert_eq!(
            health.to_string(),
            "Dex is not healthy (status: 500, error: Some(\"Internal server error\"))"
        );
    }
}

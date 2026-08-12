//! Spindle configuration management via [Figment](https://crates.io/crates/figment).
//!
//! Supports layered config from multiple sources:
//! 1. Defaults (hardcoded)
//! 2. Config file (TOML) — default path `~/.config/spindle/config.toml` or via `SPINDLE_CONFIG` env var
//! 3. Environment variables — `SPINDLE_SERVER_HOST`, `SPINDLE_DATABASE_URL`, etc.

#![allow(warnings)]
use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::Duration,
};
use thiserror::Error;

// ── Error types ────────────────────────────────────────────────────────

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("config file not found at {path}")]
    FileNotFound { path: PathBuf },

    #[error("failed to parse config file: {0}")]
    ParseFailed(String),

    #[error("missing required field: {section}.{field} — set via TOML or env SPINDLE_{0}_{1}", .section.to_uppercase(), .field.to_uppercase())]
    MissingField { section: &'static str, field: &'static str },

    #[error("invalid value for {section}.{field}: {value} — expected one of {valid}", valid = .valid.join(", "))]
    InvalidEnum {
        section: &'static str,
        field: &'static str,
        value: String,
        valid: Vec<String>,
    },

    #[error("invalid {section}.{field}: {reason}")]
    InvalidValue {
        section: &'static str,
        field: &'static str,
        reason: String,
    },

    #[error("database URL must use postgres or postgresql scheme, got: {scheme}")]
    InvalidDatabaseScheme { scheme: String },

    #[error("retention period must be at least {min:?}, got: {value:?}")]
    RetentionTooShort { min: Duration, value: Duration },

    #[error("identity mapping rule at index {index} is invalid: {reason}")]
    MappingRuleInvalid { index: usize, reason: String },

    #[error("ambiguous mapping rules at indices {rule_a_index} and {rule_b_index}: {reason}")]
    AmbiguousMappingRule {
        rule_a_index: usize,
        rule_b_index: usize,
        reason: String,
    },

    #[error("circular group reference detected: {reason}")]
    CircularGroupReference { reason: String },

    #[error("TLS error: {reason}")]
    TlsError { reason: String },
}

// ── Identity mapping rules module (M3-08) ────────────────────────────────────

pub mod mappings;
pub use mappings::{MappingEvaluator, MappingResult, MappingRule, MatchType, validate_mappings};

// ── Server config ──────────────────────────────────────────────────────

/// Server binding configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ServerConfig {
    /// Host to bind the HTTP server to (default: "127.0.0.1").
    #[serde(default = "default_host")]
    pub host: IpAddr,

    /// Port to bind the HTTP server to (default: 3000).
    #[serde(default = "default_port")]
    pub port: u16,

    /// Maximum concurrent connections (default: 1024).
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,

    /// Read timeout for inbound requests in seconds (default: 30).
    #[serde(default = "default_read_timeout_secs")]
    pub read_timeout_secs: u64,

    /// Write timeout for outbound responses in seconds (default: 60).
    #[serde(default = "default_write_timeout_secs")]
    pub write_timeout_secs: u64,

    /// Whether to enable CORS (default: false).
    #[serde(default)]
    pub cors_enabled: bool,

    /// TLS configuration. Controlled by SPINDLE_TLS_* env vars.
    #[serde(default)]
    pub tls: TlsConfig,
}

fn default_host() -> IpAddr {
    IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))
}
fn default_port() -> u16 {
    3000
}
fn default_max_connections() -> u32 {
    1024
}
fn default_read_timeout_secs() -> u64 {
    30
}
fn default_write_timeout_secs() -> u64 {
    60
}

impl ServerConfig {
    pub fn addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }
    pub fn read_timeout(&self) -> Duration {
        Duration::from_secs(self.read_timeout_secs)
    }
    pub fn write_timeout(&self) -> Duration {
        Duration::from_secs(self.write_timeout_secs)
    }
    fn validate(&self) -> Result<(), ConfigError> {
        if self.port == 0 {
            return Err(ConfigError::InvalidValue {
                section: "server",
                field: "port",
                reason: "port cannot be 0".into(),
            });
        }
        self.tls.validate()?;
        Ok(())
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            max_connections: default_max_connections(),
            read_timeout_secs: default_read_timeout_secs(),
            write_timeout_secs: default_write_timeout_secs(),
            cors_enabled: false,
            tls: TlsConfig::default(),
        }
    }
}

// ── TLS config ────────────────────────────────────────────────────────

/// TLS configuration for the HTTP server.
///
/// Env vars: `SPINDLE_TLS_ENABLED` (default: "0"/false), `SPINDLE_TLS_CERT` (path),
/// `SPINDLE_TLS_KEY` (path). In production mode (`SPINDLE_PRODUCTION=1`) TLS
/// must be enabled or the server refuses to start.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct TlsConfig {
    /// Whether TLS is enabled (default: false).
    /// Set via `SPINDLE_TLS_ENABLED=1` or `tls.enabled = true` in TOML.
    #[serde(default)]
    pub enabled: bool,

    /// Path to the TLS certificate file (PEM format).
    /// Required when TLS is enabled. Set via `SPINDLE_TLS_CERT` or `tls.cert`.
    pub cert: Option<String>,

    /// Path to the TLS private key file (PEM format).
    /// Required when TLS is enabled. Set via `SPINDLE_TLS_KEY` or `tls.key`.
    pub key: Option<String>,
}

impl TlsConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.enabled {
            if self.cert.is_none() {
                return Err(ConfigError::MissingField {
                    section: "tls",
                    field: "cert",
                });
            }
            if self.key.is_none() {
                return Err(ConfigError::MissingField {
                    section: "tls",
                    field: "key",
                });
            }
        }
        Ok(())
    }
}

// ── Database config ────────────────────────────────────────────────────

/// Database connection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DatabaseConfig {
    /// PostgreSQL connection string (required).
    pub url: String,

    /// Maximum connection pool size (default: 10).
    #[serde(default = "default_pool_max")]
    pub pool_max: u32,

    /// Minimum idle connections (default: 2).
    #[serde(default = "default_pool_min")]
    pub pool_min: u32,

    /// Connection acquisition timeout in seconds (default: 30).
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,

    /// Enable connection pool health checks (default: true).
    #[serde(default = "default_health_check")]
    pub health_check: bool,

    /// Maximum connection lifetime in seconds (default: 1800).
    #[serde(default = "default_max_lifetime_secs")]
    pub max_lifetime_secs: u64,
}

fn default_pool_max() -> u32 {
    10
}
fn default_pool_min() -> u32 {
    2
}
fn default_connect_timeout_secs() -> u64 {
    30
}
fn default_health_check() -> bool {
    true
}
fn default_max_lifetime_secs() -> u64 {
    1800
}

impl DatabaseConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.url.is_empty() {
            return Err(ConfigError::MissingField {
                section: "database",
                field: "url",
            });
        }
        let scheme = self.url.split("://").next().unwrap_or("");
        if scheme != "postgres" && scheme != "postgresql" {
            return Err(ConfigError::InvalidDatabaseScheme {
                scheme: scheme.to_string(),
            });
        }
        if self.pool_max < self.pool_min {
            return Err(ConfigError::InvalidValue {
                section: "database",
                field: "pool_max",
                reason: format!(
                    "pool_max ({}) must be >= pool_min ({})",
                    self.pool_max, self.pool_min
                ),
            });
        }
        Ok(())
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            pool_max: default_pool_max(),
            pool_min: default_pool_min(),
            connect_timeout_secs: default_connect_timeout_secs(),
            health_check: default_health_check(),
            max_lifetime_secs: default_max_lifetime_secs(),
        }
    }
}

// ── Storage config ─────────────────────────────────────────────────────

/// Object storage backend type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum StorageBackend {
    S3,
    Local,
    Gcs,
    Azure,
}

impl fmt::Display for StorageBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageBackend::S3 => write!(f, "s3"),
            StorageBackend::Local => write!(f, "local"),
            StorageBackend::Gcs => write!(f, "gcs"),
            StorageBackend::Azure => write!(f, "azure"),
        }
    }
}

/// Object storage configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct StorageConfig {
    /// Storage backend type (default: "local").
    #[serde(default = "default_storage_backend")]
    pub backend: StorageBackend,

    /// Bucket or base directory name (default: "spindle-data").
    #[serde(default = "default_bucket")]
    pub bucket: String,

    /// S3 endpoint URL (optional).
    pub endpoint: Option<String>,

    /// Region name (default: "us-east-1").
    #[serde(default = "default_region")]
    pub region: String,

    /// Access key ID (required for cloud backends).
    pub access_key_id: Option<String>,

    /// Secret access key (required for cloud backends).
    pub secret_access_key: Option<String>,

    /// Maximum upload part size in bytes (default: 5MB).
    #[serde(default = "default_max_part_size")]
    pub max_part_size_bytes: u64,

    /// Use path-style URLs (default: false).
    #[serde(default)]
    pub path_style: bool,
}

fn default_storage_backend() -> StorageBackend {
    StorageBackend::Local
}
fn default_bucket() -> String {
    "spindle-data".into()
}
fn default_region() -> String {
    "us-east-1".into()
}
fn default_max_part_size() -> u64 {
    5 * 1024 * 1024
}

impl StorageConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.bucket.is_empty() {
            return Err(ConfigError::MissingField {
                section: "storage",
                field: "bucket",
            });
        }
        if self.backend != StorageBackend::Local {
            if self.access_key_id.is_none() {
                return Err(ConfigError::MissingField {
                    section: "storage",
                    field: "access_key_id",
                });
            }
            if self.secret_access_key.is_none() {
                return Err(ConfigError::MissingField {
                    section: "storage",
                    field: "secret_access_key",
                });
            }
        }
        Ok(())
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: default_storage_backend(),
            bucket: default_bucket(),
            endpoint: None,
            region: default_region(),
            access_key_id: None,
            secret_access_key: None,
            max_part_size_bytes: default_max_part_size(),
            path_style: false,
        }
    }
}

// ── Identity config ────────────────────────────────────────────────────

/// OIDC authentication configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct IdentityConfig {
    /// OIDC issuer URL (required when enabled).
    pub issuer_url: Option<String>,

    /// Client ID (required when enabled).
    pub client_id: Option<String>,

    /// Client secret (required when enabled).
    pub client_secret: Option<String>,

    /// Redirect URI.
    pub redirect_uri: Option<String>,

    /// Expected scopes (default: ["openid", "email"]).
    #[serde(default = "default_scopes")]
    pub scopes: Vec<String>,

    /// Token refresh buffer in seconds (default: 300).
    #[serde(default = "default_refresh_buffer_secs")]
    pub refresh_buffer_secs: u64,

    /// Session timeout in seconds (default: 3600).
    #[serde(default = "default_session_timeout_secs")]
    pub session_timeout_secs: u64,

    /// Group/claim mapping rules (M3-08).
    /// Each rule maps a connector's groups or claims to internal roles and scopes.
    /// Rules are evaluated in config order; first match wins.
    #[serde(default)]
    pub mappings: Vec<MappingRule>,
}

fn default_scopes() -> Vec<String> {
    vec!["openid".into(), "email".into()]
}
fn default_refresh_buffer_secs() -> u64 {
    300
}
fn default_session_timeout_secs() -> u64 {
    3600
}

impl IdentityConfig {
    pub fn is_enabled(&self) -> bool {
        self.issuer_url.is_some() && self.client_id.is_some() && self.client_secret.is_some()
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let has_any =
            self.issuer_url.is_some() || self.client_id.is_some() || self.client_secret.is_some();
        if has_any && !self.is_enabled() {
            if self.issuer_url.is_none() {
                return Err(ConfigError::MissingField {
                    section: "identity",
                    field: "issuer_url",
                });
            }
            if self.client_id.is_none() {
                return Err(ConfigError::MissingField {
                    section: "identity",
                    field: "client_id",
                });
            }
            if self.client_secret.is_none() {
                return Err(ConfigError::MissingField {
                    section: "identity",
                    field: "client_secret",
                });
            }
        }
        // Validate mapping rules
        validate_mappings(&self.mappings)?;
        Ok(())
    }
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            issuer_url: None,
            client_id: None,
            client_secret: None,
            redirect_uri: None,
            scopes: default_scopes(),
            refresh_buffer_secs: default_refresh_buffer_secs(),
            session_timeout_secs: default_session_timeout_secs(),
            mappings: vec![],
        }
    }
}

// ── Signing config ─────────────────────────────────────────────────────

/// PGP/content signing mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SigningMode {
    Strict,
    Optional,
    Disabled,
}

impl fmt::Display for SigningMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SigningMode::Strict => write!(f, "strict"),
            SigningMode::Optional => write!(f, "optional"),
            SigningMode::Disabled => write!(f, "disabled"),
        }
    }
}

/// Content signing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SigningConfig {
    /// Signing mode (default: "disabled").
    #[serde(default = "default_signing_mode")]
    pub mode: SigningMode,

    /// Path to GPG/PGP keyring file.
    pub keyring_path: Option<String>,

    /// Key ID for signing (hex).
    pub key_id: Option<String>,

    /// Passphrase for signing key.
    pub passphrase: Option<String>,

    /// Hash algorithm (default: "sha256", valid: sha256, sha384, sha512).
    #[serde(default = "default_hash_algorithm")]
    pub hash_algorithm: String,

    /// Include public key in signed payloads (default: true).
    #[serde(default = "default_include_public_key")]
    pub include_public_key: bool,
}

fn default_signing_mode() -> SigningMode {
    SigningMode::Disabled
}
fn default_hash_algorithm() -> String {
    "sha256".into()
}
fn default_include_public_key() -> bool {
    true
}

impl SigningConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        match self.mode {
            SigningMode::Strict | SigningMode::Optional => {
                if self.keyring_path.is_none() {
                    return Err(ConfigError::MissingField {
                        section: "signing",
                        field: "keyring_path",
                    });
                }
                if self.key_id.is_none() {
                    return Err(ConfigError::MissingField {
                        section: "signing",
                        field: "key_id",
                    });
                }
            }
            SigningMode::Disabled => {}
        }
        let valid = ["sha256", "sha384", "sha512"];
        if !valid.contains(&self.hash_algorithm.as_str()) {
            return Err(ConfigError::InvalidEnum {
                section: "signing",
                field: "hash_algorithm",
                value: self.hash_algorithm.clone(),
                valid: valid.iter().map(|s| s.to_string()).collect(),
            });
        }
        Ok(())
    }
}

impl Default for SigningConfig {
    fn default() -> Self {
        Self {
            mode: default_signing_mode(),
            keyring_path: None,
            key_id: None,
            passphrase: None,
            hash_algorithm: default_hash_algorithm(),
            include_public_key: default_include_public_key(),
        }
    }
}

// ── Ingest config ──────────────────────────────────────────────────────

/// Ingestion pipeline parallelism mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum IngestParallelism {
    Sequential,
    Bounded,
    Unbounded,
}

impl fmt::Display for IngestParallelism {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IngestParallelism::Sequential => write!(f, "sequential"),
            IngestParallelism::Bounded => write!(f, "bounded"),
            IngestParallelism::Unbounded => write!(f, "unbounded"),
        }
    }
}

/// Content ingestion pipeline configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct IngestConfig {
    /// Maximum batch size (default: 100).
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,

    /// Parallelism mode (default: "bounded").
    #[serde(default = "default_parallelism")]
    pub parallelism: IngestParallelism,

    /// Max concurrent workers for bounded mode (default: 8).
    #[serde(default = "default_max_workers")]
    pub max_workers: usize,

    /// Max document size in bytes (default: 10MB).
    #[serde(default = "default_max_document_size")]
    pub max_document_size_bytes: u64,

    /// Compute content hashes on ingest (default: true).
    #[serde(default = "default_compute_hashes")]
    pub compute_hashes: bool,

    /// Retry count for failed operations (default: 3).
    #[serde(default = "default_retry_count")]
    pub retry_count: u32,

    /// Retry backoff base in ms (default: 1000).
    #[serde(default = "default_retry_backoff_ms")]
    pub retry_backoff_ms: u64,
}

fn default_batch_size() -> usize {
    100
}
fn default_parallelism() -> IngestParallelism {
    IngestParallelism::Bounded
}
fn default_max_workers() -> usize {
    8
}
fn default_max_document_size() -> u64 {
    10 * 1024 * 1024
}
fn default_compute_hashes() -> bool {
    true
}
fn default_retry_count() -> u32 {
    3
}
fn default_retry_backoff_ms() -> u64 {
    1000
}

impl IngestConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.batch_size == 0 {
            return Err(ConfigError::InvalidValue {
                section: "ingest",
                field: "batch_size",
                reason: "batch_size must be > 0".into(),
            });
        }
        if self.parallelism == IngestParallelism::Bounded && self.max_workers == 0 {
            return Err(ConfigError::InvalidValue {
                section: "ingest",
                field: "max_workers",
                reason: "max_workers must be > 0 when bounded".into(),
            });
        }
        if self.max_document_size_bytes == 0 {
            return Err(ConfigError::InvalidValue {
                section: "ingest",
                field: "max_document_size_bytes",
                reason: "must be > 0".into(),
            });
        }
        Ok(())
    }
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            batch_size: default_batch_size(),
            parallelism: default_parallelism(),
            max_workers: default_max_workers(),
            max_document_size_bytes: default_max_document_size(),
            compute_hashes: default_compute_hashes(),
            retry_count: default_retry_count(),
            retry_backoff_ms: default_retry_backoff_ms(),
        }
    }
}

// ── Archive config ────────────────────────────────────────────────────
/// Archival pipeline configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ArchiveConfig {
    /// Archive backend type: "local" or "s3" (default: "local").
    #[serde(default = "default_archive_type")]
    pub archive_type: String,

    /// Archive storage path or bucket prefix (default: "/var/lib/spindle/archive").
    #[serde(default = "default_archive_path")]
    pub path: String,

    /// Enable compression for archive bundles (default: true).
    #[serde(default = "default_archive_compression")]
    pub compression: bool,
}

fn default_archive_type() -> String {
    "local".into()
}
fn default_archive_path() -> String {
    "/var/lib/spindle/archive".into()
}
fn default_archive_compression() -> bool {
    true
}

impl ArchiveConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.archive_type != "local" && self.archive_type != "s3" {
            return Err(ConfigError::InvalidValue {
                section: "archive",
                field: "archive_type",
                reason: "archive_type must be \"local\" or \"s3\"".into(),
            });
        }
        if self.path.is_empty() {
            return Err(ConfigError::InvalidValue {
                section: "archive",
                field: "path",
                reason: "archive path must not be empty".into(),
            });
        }
        Ok(())
    }
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            archive_type: default_archive_type(),
            path: default_archive_path(),
            compression: default_archive_compression(),
        }
    }
}

// ── Observability config ───────────────────────────────────────────────

/// Three-tier log level for structured logging.
/// Maps to tracing levels: L1→info, L2→debug, L3→trace.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum LogLevel {
    /// L1 — Operational. Always on. Minimal disk, zero perf impact.
    #[default]
    Operational,
    /// L2 — Diagnostic. Opt-in. Payload metadata, per-resource breakdown,
    /// query params, per-table latency. Can fill disk, must not slow system.
    Diagnostic,
    /// L3 — Debug. Full payload bodies, full SQL, intermediate pipeline state.
    /// Disk/perf free-for-all. Never in prod.
    Debug,
}


impl LogLevel {
    /// Convert to the tracing level string used by EnvFilter.
    pub fn as_tracing_level(&self) -> &'static str {
        match self {
            LogLevel::Operational => "info",
            LogLevel::Diagnostic => "debug",
            LogLevel::Debug => "trace",
        }
    }
}

/// Observability configuration: logging tier and secret scanning.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ObservabilityConfig {
    /// Three-tier log level: "operational" (L1/info), "diagnostic" (L2/debug),
    /// "debug" (L3/trace). Default: "operational".
    #[serde(default)]
    pub log_level: LogLevel,

    /// Whether to enable secret scanning on log output.
    /// Active by default on stdout; disable for JSON/log-shipper targets.
    #[serde(default = "default_scan_secrets")]
    pub scan_secrets: bool,
}

fn default_scan_secrets() -> bool {
    true
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            log_level: LogLevel::default(),
            scan_secrets: default_scan_secrets(),
        }
    }
}

impl ObservabilityConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        Ok(())
    }
}

// ── Root config ────────────────────────────────────────────────────────

/// Full Spindle application configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    /// Server binding and HTTP settings.
    #[serde(default)]
    pub server: ServerConfig,

    /// PostgreSQL database connection.
    #[serde(default)]
    pub database: DatabaseConfig,

    /// Object storage backend.
    #[serde(default)]
    pub storage: StorageConfig,

    /// OIDC identity provider.
    #[serde(default)]
    pub identity: IdentityConfig,

    /// PGP/content signing.
    #[serde(default)]
    pub signing: SigningConfig,

    /// Ingestion pipeline.
    #[serde(default)]
    pub ingest: IngestConfig,
    /// Archival pipeline.
    #[serde(default)]
    pub archive: ArchiveConfig,

    /// Data retention and archival.
    #[serde(default)]
    pub retention: RetentionConfig,

    /// Observability / logging configuration.
    #[serde(default)]
    pub observability: ObservabilityConfig,
}

// ── Retention config ───────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RetentionConfig {
    /// Raw data retention in days (default: 90).
    #[serde(default = "default_raw_retention_days")]
    pub raw_retention_days: u64,

    /// Processed data retention in days (default: 365).
    #[serde(default = "default_processed_retention_days")]
    pub processed_retention_days: u64,

    /// Archive retention in days (0 = forever, default: 0).
    #[serde(default)]
    pub archive_retention_days: u64,

    /// Cleanup cron expression (default: "0 3 * * *").
    #[serde(default = "default_cleanup_cron")]
    pub cleanup_cron: String,

    /// Enable automatic cleanup (default: false).
    #[serde(default)]
    pub auto_cleanup: bool,

    /// Minimum retention period in days (default: 7).
    #[serde(default = "default_min_retention_days")]
    pub min_retention_days: u64,
}

fn default_raw_retention_days() -> u64 {
    90
}
fn default_processed_retention_days() -> u64 {
    365
}
fn default_cleanup_cron() -> String {
    "0 3 * * *".into()
}
fn default_min_retention_days() -> u64 {
    7
}

impl RetentionConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        let min = Duration::from_secs(self.min_retention_days * 86400);
        let raw = Duration::from_secs(self.raw_retention_days * 86400);
        let processed = Duration::from_secs(self.processed_retention_days * 86400);
        if raw < min {
            return Err(ConfigError::RetentionTooShort { min, value: raw });
        }
        if processed < min {
            return Err(ConfigError::RetentionTooShort {
                min,
                value: processed,
            });
        }
        if self.raw_retention_days > self.processed_retention_days {
            return Err(ConfigError::InvalidValue {
                section: "retention",
                field: "raw_retention_days",
                reason: format!(
                    "raw ({}) cannot exceed processed ({})",
                    self.raw_retention_days, self.processed_retention_days
                ),
            });
        }
        Ok(())
    }
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            raw_retention_days: default_raw_retention_days(),
            processed_retention_days: default_processed_retention_days(),
            archive_retention_days: 0,
            cleanup_cron: default_cleanup_cron(),
            auto_cleanup: false,
            min_retention_days: default_min_retention_days(),
        }
    }
}

impl Config {
    /// Load from defaults + file + environment.
    pub fn load() -> Result<Self, ConfigError> {
        let figment = Self::build_figment();
        let config: Self = figment
            .extract()
            .map_err(|err| ConfigError::ParseFailed(err.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    /// Load from a custom Figment.
    pub fn load_from(figment: Figment) -> Result<Self, ConfigError> {
        let config: Self = figment
            .extract()
            .map_err(|err| ConfigError::ParseFailed(err.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    /// Build the default layered Figment.
    pub fn build_figment() -> Figment {
        let config_path = std::env::var("SPINDLE_CONFIG")
            .ok()
            .or_else(|| {
                std::env::var("HOME").ok().and_then(|h| {
                    let path = PathBuf::from(h)
                        .join(".config")
                        .join("spindle")
                        .join("config.toml");
                    if path.exists() {
                        Some(path.to_string_lossy().into_owned())
                    } else {
                        None
                    }
                })
            })
            .or_else(|| {
                if PathBuf::from("config.toml").exists() {
                    Some("config.toml".into())
                } else {
                    None
                }
            });

        let mut figment = Figment::new();
        figment = figment.merge(Serialized::default("defaults", Config::default()));
        if let Some(path) = &config_path {
            figment = figment.merge(Toml::file(path));
        }
        figment = figment.admerge(Env::prefixed("SPINDLE_").split("_"));
        figment
    }

    /// Return all-default config.
    pub fn defaults() -> Self {
        Self::default()
    }

    /// Validate all sections.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.server.validate()?;
        self.database.validate()?;
        self.storage.validate()?;
        self.identity.validate()?;
        self.signing.validate()?;
        self.ingest.validate()?;
        self.retention.validate()?;
        self.observability.validate()?;
        Ok(())
    }
}

/// Create a minimal valid config for testing.
pub fn test_config() -> Config {
    Config {
        server: ServerConfig::default(),
        database: DatabaseConfig {
            url: "postgres://user:pass@localhost/spindle_test".into(),
            ..DatabaseConfig::default()
        },
        storage: StorageConfig::default(),
        identity: IdentityConfig::default(),
        signing: SigningConfig::default(),
        ingest: IngestConfig::default(),
        archive: ArchiveConfig::default(),
        retention: RetentionConfig::default(),
        observability: ObservabilityConfig::default(),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Default values ─────────────────────────────────────────

    #[test]
    fn test_server_defaults() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.host, IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)));
        assert_eq!(cfg.port, 3000);
        assert_eq!(cfg.max_connections, 1024);
        assert!(!cfg.cors_enabled);
    }

    #[test]
    fn test_storage_defaults() {
        let cfg = StorageConfig::default();
        assert_eq!(cfg.backend, StorageBackend::Local);
        assert_eq!(cfg.bucket, "spindle-data");
    }

    #[test]
    fn test_signing_defaults() {
        let cfg = SigningConfig::default();
        assert_eq!(cfg.mode, SigningMode::Disabled);
        assert_eq!(cfg.hash_algorithm, "sha256");
    }

    #[test]
    fn test_ingest_defaults() {
        let cfg = IngestConfig::default();
        assert_eq!(cfg.batch_size, 100);
        assert_eq!(cfg.parallelism, IngestParallelism::Bounded);
        assert_eq!(cfg.max_workers, 8);
    }

    #[test]
    fn test_retention_defaults() {
        let cfg = RetentionConfig::default();
        assert_eq!(cfg.raw_retention_days, 90);
        assert_eq!(cfg.processed_retention_days, 365);
        assert!(!cfg.auto_cleanup);
    }

    // ── Missing required fields ─────────────────────────────────

    #[test]
    fn test_missing_database_url() {
        let db = DatabaseConfig::default();
        assert!(matches!(
            db.validate(),
            Err(ConfigError::MissingField { .. })
        ));
    }

    #[test]
    fn test_missing_signing_keyring() {
        let mut cfg = SigningConfig::default();
        cfg.mode = SigningMode::Strict;
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::MissingField { .. }));
        assert!(err.to_string().contains("keyring_path"));
    }

    #[test]
    fn test_missing_signing_key_id() {
        let mut cfg = SigningConfig::default();
        cfg.mode = SigningMode::Optional;
        cfg.keyring_path = Some("/tmp/keyring".into());
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::MissingField { .. }));
        assert!(err.to_string().contains("key_id"));
    }

    #[test]
    fn test_identity_partial_fails() {
        let mut cfg = IdentityConfig::default();
        cfg.issuer_url = Some("https://auth.example.com".into());
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_identity_all_or_nothing() {
        assert!(IdentityConfig::default().validate().is_ok());
        let mut cfg = IdentityConfig::default();
        cfg.issuer_url = Some("https://auth.example.com".into());
        cfg.client_id = Some("my-client".into());
        cfg.client_secret = Some("my-secret".into());
        assert!(cfg.validate().is_ok());
        assert!(cfg.is_enabled());
    }

    #[test]
    fn test_cloud_storage_requires_creds() {
        let mut cfg = StorageConfig::default();
        cfg.backend = StorageBackend::S3;
        assert!(cfg.validate().is_err());
    }

    // ── Invalid enums ──────────────────────────────────────────

    #[test]
    fn test_invalid_hash_algorithm() {
        let mut cfg = SigningConfig::default();
        cfg.mode = SigningMode::Strict;
        cfg.keyring_path = Some("/tmp/keyring".into());
        cfg.key_id = Some("ABCD1234".into());
        cfg.hash_algorithm = "md5".into();
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidEnum { .. }));
        assert!(err.to_string().contains("md5"));
    }

    #[test]
    fn test_invalid_database_scheme() {
        let cfg = DatabaseConfig {
            url: "mysql://user:pass@localhost/db".into(),
            ..DatabaseConfig::default()
        };
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::InvalidDatabaseScheme { .. })
        ));
    }

    #[test]
    fn test_valid_database_schemes() {
        for scheme in ["postgres", "postgresql"] {
            let cfg = DatabaseConfig {
                url: format!("{scheme}://u:p@l/d"),
                ..DatabaseConfig::default()
            };
            assert!(cfg.validate().is_ok());
        }
    }

    // ── Value constraints ──────────────────────────────────────

    #[test]
    fn test_pool_max_lt_min() {
        let cfg = DatabaseConfig {
            url: "postgres://u:p@l/d".into(),
            pool_max: 2,
            pool_min: 5,
            ..DatabaseConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_zero_port() {
        let mut cfg = ServerConfig::default();
        cfg.port = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_zero_batch_size() {
        let mut cfg = IngestConfig::default();
        cfg.batch_size = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_zero_workers_bounded() {
        let mut cfg = IngestConfig::default();
        cfg.max_workers = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_retention_raw_exceeds_processed() {
        let cfg = RetentionConfig {
            raw_retention_days: 400,
            processed_retention_days: 365,
            ..RetentionConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_retention_below_minimum() {
        let cfg = RetentionConfig {
            raw_retention_days: 3,
            min_retention_days: 7,
            ..RetentionConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    // ── Env overrides (via std::env) ────────────────────────────

    use std::sync::Mutex;

    /// Global mutex to serialize env-var tests (std::env is process-wide).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Helper: run a closure with specific SPINDLE_* env vars set,
    /// isolated from other parallel tests.
    fn with_env<F: FnOnce() -> T, T>(vars: &[(&str, &str)], f: F) -> T {
        let _guard = ENV_LOCK.lock().unwrap();

        // Save existing SPINDLE_* vars
        let existing: Vec<_> = std::env::vars()
            .filter(|(k, _)| k.starts_with("SPINDLE_"))
            .map(|(k, v)| (k, Some(v)))
            .collect();

        // Clear all SPINDLE_* vars
        for (k, _) in &existing {
            std::env::remove_var(k);
        }

        // Set new vars
        for (k, v) in vars {
            std::env::set_var(k, v);
        }

        let result = f();

        // Clean up: remove all SPINDLE_* vars we set
        for (k, _) in vars {
            std::env::remove_var(k);
        }
        // Restore pre-existing ones
        for (k, v) in &existing {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }

        result
    }

    /// Helper to run a test with a TOML file + optional env overrides.
    fn with_toml<F: FnOnce() -> T, T>(toml_content: &str, env_vars: &[(&str, &str)], f: F) -> T {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), toml_content).unwrap();

        let mut vars: Vec<(&str, &str)> =
            vec![("SPINDLE_CONFIG", tmp.path().to_str().unwrap())];
        vars.extend(env_vars.iter().map(|(k, v)| (*k, *v)));

        with_env(&vars, f)
    }

    #[test]
    fn test_env_override_server_port() {
        let cfg = with_env(
            &[
                ("SPINDLE_SERVER_PORT", "9090"),
                ("SPINDLE_DATABASE_URL", "postgres://u:p@l/d"),
            ],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate().unwrap();
                cfg
            },
        );
        assert_eq!(cfg.server.port, 9090);
    }

    #[test]
    fn test_env_override_database_url() {
        let cfg = with_env(
            &[("SPINDLE_DATABASE_URL", "postgres://admin:p@db.host/spindle_prod")],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate().unwrap();
                cfg
            },
        );
        assert_eq!(cfg.database.url, "postgres://admin:p@db.host/spindle_prod");
    }

    #[test]
    fn test_env_override_signing_mode() {
        // "mode" is a single-word field, works with split("_")
        let cfg = with_env(
            &[
                ("SPINDLE_SIGNING_MODE", "strict"),
                ("SPINDLE_DATABASE_URL", "postgres://u:p@l/d"),
            ],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                // signing.mode = Strict needs keyring_path and key_id
                cfg
            },
        );
        assert_eq!(cfg.signing.mode, SigningMode::Strict);
        // Validation fails because keyring_path/key_id are missing
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_env_override_ingest_parallelism() {
        let cfg = with_env(
            &[
                ("SPINDLE_INGEST_PARALLELISM", "sequential"),
                ("SPINDLE_DATABASE_URL", "postgres://u:p@l/d"),
            ],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate().unwrap();
                cfg
            },
        );
        assert_eq!(cfg.ingest.parallelism, IngestParallelism::Sequential);
    }

    // ── TOML file loading ──────────────────────────────────────

    #[test]
    fn test_toml_load_full_config() {
        let cfg = with_toml(
            r#"
[server]
host = "0.0.0.0"
port = 8080
cors-enabled = true

[database]
url = "postgres://spindle:p@localhost/spindle"
pool-max = 50
pool-min = 5

[storage]
backend = "s3"
bucket = "my-spindle-bucket"
region = "eu-west-1"
access-key-id = "AKIAIOSFODNN7EXAMPLE"
secret-access-key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
path-style = true

[signing]
mode = "optional"
keyring-path = "/home/spindle/.gnupg/pubring.kbx"
key-id = "CAFEBEEF"
hash-algorithm = "sha512"

[ingest]
batch-size = 200
parallelism = "bounded"
max-workers = 16

[retention]
raw-retention-days = 60
processed-retention-days = 730
auto-cleanup = true
"#,
            &[],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate().unwrap();
                cfg
            },
        );
        assert_eq!(cfg.server.port, 8080);
        assert!(cfg.server.cors_enabled);
        assert_eq!(cfg.database.pool_max, 50);
        assert_eq!(cfg.storage.backend, StorageBackend::S3);
        assert_eq!(cfg.storage.bucket, "my-spindle-bucket");
        assert!(cfg.storage.path_style);
        assert_eq!(cfg.signing.mode, SigningMode::Optional);
        assert_eq!(cfg.signing.hash_algorithm, "sha512");
        assert_eq!(cfg.ingest.batch_size, 200);
        assert_eq!(cfg.ingest.max_workers, 16);
        assert_eq!(cfg.retention.raw_retention_days, 60);
        assert!(cfg.retention.auto_cleanup);
    }

    #[test]
    fn test_toml_partial_merge() {
        let cfg = with_toml(
            r#"
[server]
port = 4000

[database]
url = "postgres://u:p@l/d"
"#,
            &[],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate().unwrap();
                cfg
            },
        );
        assert_eq!(cfg.server.port, 4000);
        assert_eq!(cfg.server.max_connections, 1024);
        assert_eq!(cfg.ingest.batch_size, 100);
    }

    #[test]
    fn test_env_overrides_toml() {
        let cfg = with_toml(
            r#"
[server]
port = 4000

[database]
url = "postgres://u:p@l/d"
"#,
            &[("SPINDLE_SERVER_PORT", "5000")],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate().unwrap();
                cfg
            },
        );
        assert_eq!(cfg.server.port, 5000);
    }

    #[test]
    fn test_env_override_toml_db_url() {
        let cfg = with_toml(
            r#"
[database]
url = "postgres://u:p@l/d"
"#,
            &[("SPINDLE_DATABASE_URL", "postgres://override:p@other.host/prod")],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate().unwrap();
                cfg
            },
        );
        assert_eq!(cfg.database.url, "postgres://override:p@other.host/prod");
    }

    #[test]
    fn test_toml_s3_storage() {
        let cfg = with_toml(
            r#"
[database]
url = "postgres://u:p@l/d"

[storage]
backend = "s3"
bucket = "my-bucket"
access-key-id = "AKIAIOSFODNN7EXAMPLE"
secret-access-key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
"#,
            &[],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate().unwrap();
                cfg
            },
        );
        assert_eq!(cfg.storage.backend, StorageBackend::S3);
        assert_eq!(cfg.storage.bucket, "my-bucket");
    }

    #[test]
    fn test_toml_signing_strict() {
        let cfg = with_toml(
            r#"
[database]
url = "postgres://u:p@l/d"

[signing]
mode = "strict"
keyring-path = "/etc/spindle/keyring"
key-id = "DEADBEEF"
"#,
            &[],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate().unwrap();
                cfg
            },
        );
        assert_eq!(cfg.signing.mode, SigningMode::Strict);
    }

    #[test]
    fn test_toml_identity_enabled() {
        let cfg = with_toml(
            r#"
[database]
url = "postgres://u:p@l/d"

[identity]
issuer-url = "https://accounts.google.com"
client-id = "my-client-id"
client-secret = "my-secret"
"#,
            &[],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate().unwrap();
                cfg
            },
        );
        assert!(cfg.identity.is_enabled());
    }

    #[test]
    fn test_toml_identity_mappings() {
        let cfg = with_toml(
            r#"
[database]
url = "postgres://u:p@l/d"

[[identity.mappings]]
connector = "ldap"
match-type = "group"
match_value = "^admin$"
assign-roles = ["viewer"]
assign-scope = ["project-admin"]

[[identity.mappings]]
connector = "oidc"
match-type = "claim"
claim-key = "department"
match_value = "^engineering$"
assign-roles = ["viewer", "compliance-auditor"]
assign-scope = ["project-engineering"]
"#,
            &[],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate().unwrap();
                cfg
            },
        );
        assert_eq!(cfg.identity.mappings.len(), 2);
        assert_eq!(cfg.identity.mappings[0].connector, "ldap");
        assert_eq!(cfg.identity.mappings[0].match_type, MatchType::Group);
        assert_eq!(cfg.identity.mappings[0].match_value, "^admin$");
        assert_eq!(cfg.identity.mappings[1].connector, "oidc");
        assert_eq!(cfg.identity.mappings[1].match_type, MatchType::Claim);
        assert_eq!(cfg.identity.mappings[1].claim_key, "department");
    }

    #[test]
    fn test_toml_identity_mappings_with_evaluator() {
        let cfg = with_toml(
            r#"
[database]
url = "postgres://u:p@l/d"

[[identity.mappings]]
connector = "ldap"
match-type = "group"
match_value = "^admin$"
assign-roles = ["viewer"]
assign-scope = ["project-admin"]

[[identity.mappings]]
connector = "ldap"
match-type = "claim"
claim-key = "department"
match_value = "^engineering$"
assign-roles = ["editor"]
assign-scope = ["project-engineering"]
"#,
            &[],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate().unwrap();
                cfg
            },
        );

        let mut evaluator = MappingEvaluator::try_new(cfg.identity.mappings.clone()).unwrap();
        let groups = vec!["admin".to_string()];
        let claims: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let result = evaluator.evaluate("ldap", "user1", &groups, &claims);
        assert_eq!(result.roles, vec!["viewer"]);
        assert_eq!(result.scope, vec!["project-admin"]);
    }

    #[test]
    fn test_config_validate_rejects_ambiguous_mappings() {
        let result = with_toml(
            r#"
[database]
url = "postgres://u:p@l/d"

[[identity.mappings]]
connector = "ldap"
match-type = "group"
match_value = ".*"
assign-roles = ["admin"]

[[identity.mappings]]
connector = "ldap"
match-type = "group"
match_value = ".*"
assign-roles = ["viewer"]
"#,
            &[],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate()
            },
        );
        // Should fail validation due to ambiguous/equivalent rules
        assert!(result.is_err());
    }

    #[test]
    fn test_config_validate_rejects_circular_mappings() {
        let result = with_toml(
            r#"
[database]
url = "postgres://u:p@l/d"

[[identity.mappings]]
connector = "ldap"
match-type = "group"
match_value = "group_a"
assign-roles = ["group_b"]

[[identity.mappings]]
connector = "ldap"
match-type = "group"
match_value = "group_b"
assign-roles = ["group_a"]
"#,
            &[],
            || {
                let figment = Config::build_figment();
                let cfg: Config = figment.extract().unwrap();
                cfg.validate()
            },
        );
        // Should fail validation due to circular group reference
        assert!(result.is_err());
    }

    #[test]
    fn test_serde_roundtrip() {
        let cfg = test_config();
        let json = serde_json::to_string(&cfg).unwrap();
        let recovered: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg.server.port, recovered.server.port);
        assert_eq!(cfg.database.url, recovered.database.url);
        assert_eq!(cfg.storage.backend, recovered.storage.backend);
    }

    #[test]
    fn test_kebab_case_serde() {
        let toml_str = r#"
[server]
cors-enabled = true

[storage]
backend = "local"
path-style = true

[ingest]
batch-size = 50
compute-hashes = false

[retention]
auto-cleanup = true
"#;
        let fig = Figment::from(Toml::string(toml_str));
        let cfg: Config = fig.extract().unwrap();
        assert!(cfg.server.cors_enabled);
        assert!(cfg.storage.path_style);
        assert!(!cfg.ingest.compute_hashes);
        assert!(cfg.retention.auto_cleanup);
    }

    // ── Helpers ────────────────────────────────────────────────

    #[test]
    fn test_server_addr() {
        let cfg = ServerConfig::default();
        assert_eq!(
            cfg.addr(),
            SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)), 3000)
        );
    }

    #[test]
    fn test_identity_is_enabled() {
        let cfg = IdentityConfig::default();
        assert!(!cfg.is_enabled());
        let mut cfg = cfg.clone();
        cfg.issuer_url = Some("https://auth.example.com".into());
        assert!(!cfg.is_enabled());
        cfg.client_id = Some("client".into());
        assert!(!cfg.is_enabled());
        cfg.client_secret = Some("secret".into());
        assert!(cfg.is_enabled());
    }

    #[test]
    fn test_config_validate_all_ok() {
        assert!(test_config().validate().is_ok());
    }

    #[test]
    fn test_error_messages_actionable() {
        let err = ConfigError::MissingField {
            section: "database",
            field: "url",
        };
        let msg = err.to_string();
        assert!(msg.contains("database"));
        assert!(msg.contains("url"));
        assert!(msg.contains("SPINDLE_DATABASE_URL"));
    }

    #[test]
    fn test_enum_display() {
        assert_eq!(format!("{}", StorageBackend::S3), "s3");
        assert_eq!(format!("{}", StorageBackend::Local), "local");
        assert_eq!(format!("{}", SigningMode::Strict), "strict");
        assert_eq!(format!("{}", SigningMode::Disabled), "disabled");
        assert_eq!(
            format!("{}", IngestParallelism::Sequential),
            "sequential"
        );
    }
}

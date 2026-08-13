// -- M4-08: Rate Limiting + Audit -----------------------------------------

use lazy_static::lazy_static;
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// -- Token Bucket ----------------------------------------------------------

struct TokenBucket {
    rate_per_minute: f64,
    burst_size: u32,
    tokens: f64,
    last_refill: std::time::Instant,
}

impl TokenBucket {
    fn new(rate_per_min: f64, burst_size: u32) -> Self {
        Self {
            rate_per_minute: rate_per_min,
            burst_size,
            tokens: rate_per_min, // start with rate tokens, accumulate up to burst
            last_refill: std::time::Instant::now(),
        }
    }

    fn consume(&mut self) -> bool {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens =
            (self.tokens + elapsed * (self.rate_per_minute / 60.0)).min(self.burst_size as f64);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

// -- Global State ----------------------------------------------------------

lazy_static! {
    static ref KEY_BUCKETS: Mutex<HashMap<String, TokenBucket>> = Mutex::new(HashMap::new());
    static ref AUDIT_LOG: Mutex<Vec<AuditEntry>> = Mutex::new(Vec::new());
}

// -- Audit Types -----------------------------------------------------------

/// Audit log entry for every sign operation.
#[derive(Clone, Debug)]
pub struct AuditEntry {
    pub timestamp: u64,
    pub key_id: String,
    pub artifact_type: String,
    pub data_hash: String,
    pub result: String,
    pub duration_ms: f64,
}

// -- Constants -------------------------------------------------------------

/// Minimum audit log retention: 1 year in seconds.
pub const AUDIT_RETENTION_SECONDS: u64 = 365 * 24 * 60 * 60;

// -- Rate Limit Config -----------------------------------------------------

fn get_rate_config() -> (f64, u32) {
    let rate: f64 = std::env::var("SPINDLE_SIGNING_RATE_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100.0);
    let burst = (rate * 10.0) as u32; // 10x burst allowance for batch exports
    (rate, burst)
}

// -- Public API ------------------------------------------------------------

/// Check and consume a rate limit token for the given key.
///
/// Returns `true` if the operation is allowed, `false` if rate-limited.
/// Also logs the attempt to the audit log.
pub fn check_rate_limit(key_id: &str) -> bool {
    let mut buckets = KEY_BUCKETS.lock().unwrap_or_else(|e| e.into_inner());
    let (rate, burst) = get_rate_config();
    let bucket = buckets
        .entry(key_id.to_string())
        .or_insert_with(|| TokenBucket::new(rate, burst));
    let allowed = bucket.consume();

    // Log the rate check to the audit log
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let entry = AuditEntry {
        timestamp,
        key_id: key_id.to_string(),
        artifact_type: "rate_check".to_string(),
        data_hash: String::new(),
        result: if allowed {
            "success".to_string()
        } else {
            "rate_limited".to_string()
        },
        duration_ms: 0.0,
    };
    AUDIT_LOG
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(entry);

    allowed
}

/// Compute MD5 hex digest of data.
fn hash_data(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    let result = hasher.finalize();
    hex::encode(result)
}

/// Log a sign attempt to the audit log.
pub fn log_sign_attempt(
    key_id: &str,
    artifact_type: &str,
    data: &[u8],
    success: bool,
    duration_ms: f64,
) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let entry = AuditEntry {
        timestamp,
        key_id: key_id.to_string(),
        artifact_type: artifact_type.to_string(),
        data_hash: hash_data(data),
        result: if success {
            "success".to_string()
        } else {
            "rate_limited".to_string()
        },
        duration_ms,
    };
    AUDIT_LOG
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(entry);
}

/// Query the audit log with optional filters.
///
/// All filters are optional; passing `None` means "don't filter on this field".
pub fn query_audit_log(
    key_id: Option<&str>,
    result: Option<&str>,
    artifact_type: Option<&str>,
) -> Vec<AuditEntry> {
    let log = AUDIT_LOG.lock().unwrap_or_else(|e| e.into_inner());
    let mut filtered: Vec<AuditEntry> = log.iter().cloned().collect();

    if let Some(k) = key_id {
        filtered.retain(|e| e.key_id == k);
    }
    if let Some(r) = result {
        filtered.retain(|e| e.result == r);
    }
    if let Some(a) = artifact_type {
        filtered.retain(|e| e.artifact_type == a);
    }

    filtered
}

/// Clear all global state (rate limiter buckets + audit log).
/// Intended for use in tests only.
pub fn clear_for_testing() {
    KEY_BUCKETS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    AUDIT_LOG.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

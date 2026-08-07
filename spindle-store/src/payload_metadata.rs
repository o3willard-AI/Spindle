use chrono::DateTime;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Payload metadata stored with every archived payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayloadMetadata {
    pub receipt_timestamp: DateTime<chrono::Utc>,
    pub source_token: String,
    pub content_type: String,
    pub payload_size: u64,
    pub payload_sha256: String,
}

impl PayloadMetadata {
    pub fn new(
        receipt_timestamp: DateTime<chrono::Utc>,
        source_token: String,
        content_type: String,
        payload: &[u8],
    ) -> Self {
        let hash = {
            let mut hasher = Sha256::new();
            hasher.update(payload);
            hasher.finalize()
        };
        Self {
            receipt_timestamp,
            source_token,
            content_type,
            payload_size: payload.len() as u64,
            payload_sha256: hex::encode(hash),
        }
    }

    /// Get the date string for key generation.
    pub fn date_str(&self) -> String {
        self.receipt_timestamp.format("%Y%m%d").to_string()
    }
}

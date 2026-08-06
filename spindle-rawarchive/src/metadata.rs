use chrono::DateTime;
use serde::{Deserialize, Serialize};

/// Metadata stored alongside payload in metadata store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveMetadata {
    pub payload_sha256: String,
    pub content_type: String,
    pub source_token: String,
    pub receipt_timestamp: DateTime<chrono::Utc>,
}

impl ArchiveMetadata {
    pub fn new(
        payload_sha256: String,
        content_type: String,
        source_token: String,
        receipt_timestamp: DateTime<chrono::Utc>,
    ) -> Self {
        Self {
            payload_sha256,
            content_type,
            source_token,
            receipt_timestamp,
        }
    }

    /// Get the date string for key construction (YYYY-MM-DD)
    pub fn date_str(&self) -> String {
        self.receipt_timestamp.format("%Y-%m-%d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_creation() {
        let meta = ArchiveMetadata::new(
            "test_digest".to_string(),
            "application/json".to_string(),
            "test_token".to_string(),
            chrono::Utc::now(),
        );

        assert_eq!(meta.payload_sha256, "test_digest");
        assert_eq!(meta.content_type, "application/json");
        assert_eq!(meta.source_token, "test_token");
        assert!(meta.date_str().len() == 10); // YYYY-MM-DD
    }
}

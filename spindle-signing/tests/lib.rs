use spindle_signing::rate_limit::{check_rate_limit, log_sign_attempt, query_audit_log};

// Serialize the rate-limit tests, which read/write the process-global
// SPINDLE_SIGNING_RATE_LIMIT env var and shared KEY_BUCKETS/AUDIT_LOG state.
lazy_static::lazy_static! {
    static ref RATE_LIMIT_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
}

// Test: exceed rate limit → 429
#[test]
fn test_rate_limit_exceeded() {
    let _guard = RATE_LIMIT_TEST_MUTEX.lock().unwrap();
    // Set environment variable for rate limit
    std::env::set_var("SPINDLE_SIGNING_RATE_LIMIT", "2");

    // First two attempts should succeed (rate limit is 2/min, burst allows 2)
    assert!(check_rate_limit("rl_test_key"));
    assert!(check_rate_limit("rl_test_key"));

    // Third attempt should be rate limited
    assert!(!check_rate_limit("rl_test_key"));

    // Query audit log for this key
    let logs = query_audit_log(Some("rl_test_key"), None, None);
    assert_eq!(logs.len(), 3, "Expected 3 audit log entries");

    // Verify the third attempt was marked as rate_limited
    assert_eq!(
        logs[2].result, "rate_limited",
        "Third attempt should be rate limited"
    );
}

// Test: audit log records sign attempt
#[test]
fn test_audit_log_records_sign_attempt() {
    let data = b"test data for signing";

    // Make a sign attempt
    log_sign_attempt("audit_key", "export", data, true, 5.0);

    // Query audit log
    let logs = query_audit_log(Some("audit_key"), None, None);
    assert_eq!(logs.len(), 1, "Expected one audit log entry");

    let entry = &logs[0];
    assert_eq!(entry.key_id, "audit_key");
    assert_eq!(entry.artifact_type, "export");
    assert_eq!(entry.result, "success");
    assert!(entry.duration_ms > 0.0);

    // Verify data hash is computed correctly
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(data);
    let expected_hash = hex::encode(hasher.finalize());
    assert_eq!(entry.data_hash, expected_hash);
}

// Test: batch export within burst allowance succeeds
#[test]
fn test_batch_export_burst_allowance() {
    let _guard = RATE_LIMIT_TEST_MUTEX.lock().unwrap();
    std::env::set_var("SPINDLE_SIGNING_RATE_LIMIT", "100");
    let data = b"batch export data";

    // Test burst allowance for 10 weekly exports at once
    // The burst size is calculated as rate * 10, so for 100/min, burst is 1000
    // We'll test with 10 exports
    for i in 0..10 {
        assert!(
            check_rate_limit("batch_key"),
            "Batch export {} should succeed",
            i
        );
    }

    // We need to call log_sign_attempt to log the attempts
    for _i in 0..10 {
        log_sign_attempt("batch_key", "weekly_export", data, true, 15.0);
    }

    // Query audit log for weekly_export entries
    let logs = query_audit_log(Some("batch_key"), None, Some("weekly_export"));
    assert_eq!(
        logs.len(),
        10,
        "Expected 10 audit log entries for batch export"
    );

    // All should be successful
    for log in &logs {
        assert_eq!(
            log.result, "success",
            "Batch export should succeed within burst allowance"
        );
    }
}

// Test: audit log queryability
#[test]
fn test_audit_log_query() {
    let data = b"query test data";

    // Create different types of entries
    log_sign_attempt("query_key", "manifest", data, true, 10.0);
    log_sign_attempt("query_key", "checkpoint", data, true, 12.0);
    log_sign_attempt("other_key", "manifest", data, true, 8.0);

    // Test querying by key_id
    let by_key = query_audit_log(Some("query_key"), None, None);
    assert_eq!(by_key.len(), 2, "Expected 2 entries for query_key");

    // Test querying by artifact_type
    let by_type = query_audit_log(None, None, Some("manifest"));
    assert_eq!(by_type.len(), 2, "Expected 2 entries for manifest type");

    // Test querying by both key_id and artifact_type
    let by_both = query_audit_log(Some("query_key"), None, Some("manifest"));
    assert_eq!(
        by_both.len(),
        1,
        "Expected 1 entry for query_key and manifest"
    );
}

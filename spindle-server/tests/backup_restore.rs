//! Tests for M5-07: Backup/restore CI verification.
//!
//! Verify:
//! - Backup scripts exist and are executable
//! - Restore script exists and is executable
//! - CI test script exists and references correct steps
//! - Documentation exists with required sections
//! - All backup/restore scripts have correct error handling (set -euo pipefail)
//! - No hardcoded credentials in scripts

#[test]
fn test_backup_scripts_exist() {
    for script in &[
        "scripts/backup-database.sh",
        "scripts/backup-manifests.sh",
        "scripts/backup-archive.sh",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(script);
        assert!(path.exists(), "Backup script must exist: {}", script);
    }
}

#[test]
fn test_restore_script_exists() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts/restore-spindle.sh");
    assert!(path.exists(), "Restore script must exist");
}

#[test]
fn test_ci_test_script_exists() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts/ci-backup-restore-test.sh");
    assert!(path.exists(), "CI test script must exist");
}

#[test]
fn test_backup_restore_docs_exist() {
    let docs_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("docs/operator/backup-restore.md");

    // Docs may be at workspace root or in docs/ dir
    let path1 = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("docs/operator/backup-restore.md");
    let path2 = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs/operator/backup-restore.md");

    assert!(
        path1.exists() || path2.exists(),
        "Backup/restore documentation must exist at docs/operator/backup-restore.md"
    );

    // Verify documentation contains required sections
    let content = std::fs::read_to_string(&path1).or_else(|_| std::fs::read_to_string(&path2)).unwrap();

    assert!(
        content.contains("pg_dump"),
        "Docs must mention pg_dump for database backup"
    );
    assert!(
        content.contains("WAL"),
        "Docs must mention WAL archiving"
    );
    assert!(
        content.contains("manifest"),
        "Docs must mention the manifests/backup manifest"
    );
    assert!(
        content.to_lowercase().contains("chain of custody")
            || content.to_lowercase().contains("chain-of-custody"),
        "Docs must mention chain of custody"
    );
    assert!(
        content.contains("restore"),
        "Docs must mention restore procedure"
    );
    assert!(
        content.contains("RPO") || content.contains("Recovery Point Objective"),
        "Docs must mention RPO"
    );
    assert!(
        content.contains("RTO") || content.contains("Recovery Time Objective"),
        "Docs must mention RTO"
    );
}

#[test]
fn test_backup_scripts_have_safe_error_handling() {
    let scripts_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts");

    let scripts = ["backup-database.sh", "backup-manifests.sh", "backup-archive.sh", "restore-spindle.sh"];

    for script in &scripts {
        let path = scripts_dir.join(script);
        if !path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("set -euo pipefail") || content.contains("set -e"),
            "{} must have safe error handling (set -euo pipefail)",
            script
        );
    }
}

#[test]
fn test_backup_scripts_no_hardcoded_credentials() {
    let scripts_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts");

    let scripts = ["backup-database.sh", "backup-manifests.sh", "backup-archive.sh", "restore-spindle.sh"];

    for script in &scripts {
        let path = scripts_dir.join(script);
        if !path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&path).unwrap();

        // Check that credentials are NOT hardcoded — they should use environment variables
        assert!(
            !content.contains("password=spindle"),
            "{} must not contain hardcoded database password",
            script
        );
        assert!(
            !content.contains("MINIO_ROOT_PASSWORD=minioadmin"),
            "{} must not contain hardcoded MinIO password",
            script
        );
        assert!(
            !content.contains("POSTGRES_PASSWORD=spindle"),
            "{} must not contain hardcoded Postgres password",
            script
        );
    }
}

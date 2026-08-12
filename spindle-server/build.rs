// Build script for spindle-server.
// Embeds the git commit SHA (short, 8 chars) and build date into the binary
// so that `--version` can report them at runtime.

fn main() {
    // Git SHA — use `git describe` for a robust value; fall back to env var
    // set by CI (e.g., SPINDLE_GIT_SHA), then to "unknown".
    let git_sha = std::env::var("SPINDLE_GIT_SHA").unwrap_or_else(|_| {
        std::process::Command::new("git")
            .args(["describe", "--always", "--abbrev=8", "--tags"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout).ok()
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "unknown".to_string())
    });

    let git_sha = if git_sha.is_empty() {
        "unknown"
    } else {
        git_sha.trim()
    };

    // Build date: use SOURCE_DATE_EPOCH for reproducible builds, otherwise
    // generate a UTC timestamp from the build machine's clock.
    let build_date = std::env::var("SOURCE_DATE_EPOCH")
        .map(|epoch| format!("epoch-{}", epoch))
        .unwrap_or_else(|_| {
            // Fall back to generating a timestamp from the system clock
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if now == 0 {
                "unknown".to_string()
            } else {
                format!("epoch-{}", now)
            }
        });

    println!("cargo:rustc-env=SPINDLE_GIT_SHA={}", git_sha);
    println!("cargo:rustc-env=SPINDLE_BUILD_DATE={}", build_date);

    // Re-run if HEAD changes.
    println!("cargo:rerun-if-changed=.git/HEAD");
}

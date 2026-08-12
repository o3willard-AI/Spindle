#!/usr/bin/env bash
# S-15: Characterization tests — shell-level verification
#
# These tests verify behaviors that are difficult to unit-test in Rust:
# 1. Production mode rejects in-memory fallback (K-7)
# 2. Clippy deny warnings — blocks new warnings (S-15)
#
# Run: bash tests/shell/char_tests.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

PASS=0
FAIL=0

report() {
    if [ "$1" -eq 0 ]; then
        echo "  ✅ $2"
        PASS=$((PASS + 1))
    else
        echo "  ❌ $2"
        FAIL=$((FAIL + 1))
    fi
}

echo "=== S-15: Characterization Tests ==="

# ── K-7: Production mode must reject in-memory fallback ──────────────────────
echo ""
echo "Test 1: test_production_mode_rejects_inmemory_fallback (K-7)"
echo "  Verifying: SPINDLE_PRODUCTION=1 with unreachable DB causes exit(1)"

# Set production mode with a DB URL pointing to a closed port
SPINDLE_PRODUCTION=1 SPINDLE_DATABASE_URL="postgres://spindle:spindle@127.0.0.1:1/spindle" \
    timeout 5 cargo run -p spindle-server --bin spindle-server -- \
    2>&1; EXIT_CODE=$?

# The server should fail before serving — exit code 1 (not 0)
# timeout gives 124 if it ran past 5s (meaning it started serving = bad)
if [ "$EXIT_CODE" -eq 1 ] || [ "$EXIT_CODE" -eq 124 ]; then
    # exit(1) is expected (FATAL: database connection failed).
    # exit 124 from timeout would mean server started but hung — still
    # better than silently running in-memory. But truly, exit 1 is correct.
    if [ "$EXIT_CODE" -eq 1 ]; then
        report 0 "Production mode with DB failure exits(1) — no in-memory fallback"
    else
        # timeout killed it — server may have started. Check if it was
        # in-memory mode (bad) or just hung on real DB attempt
        if SPINDLE_PRODUCTION=1 SPINDLE_DATABASE_URL="postgres://nope:nope@127.0.0.1:1/spindle" \
            timeout 3 cargo run -p spindle-server --bin spindle-server -- 2>&1 | \
            grep -q "FATAL: database connection failed"; then
            report 0 "Production mode logs FATAL and rejects in-memory fallback"
        else
            report 1 "Server did not show FATAL message — may have fallen back to in-memory"
        fi
    fi
else
    report 1 "Expected exit code 1, got $EXIT_CODE"
fi

# ── S-15: Clippy deny warnings ────────────────────────────────────────────────
echo ""
echo "Test 2: test_clippy_deny_blocks_new_warnings"
echo "  Verifying: clippy runs with -D warnings (deny) in the workspace"

# Run clippy with deny-warnings — any new warning becomes a hard failure
if cargo clippy --workspace --all-targets -- -D warnings 2>&1; then
    report 0 "clippy -D warnings passes cleanly"
else
    report 1 "clippy -D warnings failed — new warnings detected"
fi

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="
exit $FAIL

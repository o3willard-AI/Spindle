#!/usr/bin/env bash
# S-15: Characterization test — clippy deny warnings
#
# This test verifies that `cargo clippy` with `-D warnings` (deny) passes
# cleanly across the workspace. Any new warning becomes a hard error,
# preventing clippy from degrading over time.
#
# Run: bash tests/shell/test_clippy_deny_warnings.sh
set -euo pipefail

cd "$(cd "$(dirname "$0")" && pwd)/../.."

echo "=== S-15: test_clippy_deny_blocks_new_warnings ==="
echo "Running: cargo clippy --workspace --all-targets -- -D warnings"
echo ""

if cargo clippy --workspace --all-targets -- -D warnings 2>&1; then
    echo "✅ clippy -D warnings passes cleanly — no new warnings"
    exit 0
else
    echo "❌ clippy -D warnings failed — new warnings detected"
    exit 1
fi

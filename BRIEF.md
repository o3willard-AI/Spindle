# M1-02: Local FS backend for Archive trait

**Status:** M1-01 ✅ complete. spindle-store compile errors FIXED (Hephaestus, 2026-08-06). All 8 tests pass. Jump straight to M1-02.

## M1-02: Build local FS Archive backend
**Requirements:** RAW-04
**What:** Local filesystem implementation of `spindle-rawarchive::Archive` trait using `object_store`'s local backend.

**Key points:**
- Configurable root directory
- Same key structure as S3: `{date}/{digest}.json.gz`
- Directory-per-date for filesystem-friendliness
- Path traversal prevention (reject `../` in keys)

**Tests:**
1. Store → retrieve → byte-identical
2. Survives process restart (write, restart, read back)
3. Path traversal rejection (`../` in key → error)
4. Directory permissions correct
5. Race-free atomic writes (write-then-rename)

**Verify:** `cargo test -p spindle-rawarchive -p spindle-store` — all green

**After completion:** Commit with message "M1-02: Local filesystem Archive backend" and push.

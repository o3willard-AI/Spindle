# ADR-002: Dead Code Removal

## Status
Accepted

## Context

The Spindle codebase had accumulated dead code that was broken, never wired
into the build, and superseded by newer implementations. This ADR documents
what was removed and why.

## Decision

### Removed Files and Crates

1. **`spindle-server/src/auth.rs` (1,828 LOC)**
   - **Why removed**: This was an early authentication module that was never
     wired into the module tree (`mod auth;` was never declared in any source
     file). It was broken (compilation errors, outdated API assumptions) and
     superseded by `spindle-server/src/jit_auth.rs`, which provides JIT OIDC
     provisioning via Dex.
   - **Impact**: Zero — the code was dead. No imports, no references.

2. **`spindle-corpus-capture/` crate**
   - **Why removed**: TODO placeholder crate with no real implementation.
     Only referenced in `Cargo.toml` workspace members and as a dependency.
   - **Impact**: Zero — stub crate.

3. **`spindle-tokens/` crate**
   - **Why removed**: TODO placeholder crate with no real implementation.
   - **Impact**: Zero — stub crate.

4. **`spindle-ingest/` crate**
   - **Why removed**: TODO placeholder crate with no real implementation.
   - **Impact**: Zero — stub crate.

### Removed Binary Artifacts (from git tracking)

1. `releases/spindle-bundle-v0.1.0.tar.gz` — binary release tarball
2. `tools/evidence-collector/src/evidence_collector/__pycache__/*.pyc` — Python bytecode
3. `tools/evidence-collector/output/` directory — evidence collector output

### Updated `.gitignore`

Added patterns to prevent future tracking of:
- `releases/*.tar.gz`
- `**/*.pyc` and `__pycache__/`
- `tools/evidence-collector/output/`
- `build/spindle-bundle/bin/` and `build/spindle-bundle/etc/`

## Consequences

- Workspace now compiles with 23 crates (was 26 before stub removal).
- Git history retains the deleted files (via git history) for audit purposes.
- `.gitignore` prevents accidental re-introduction of binary artifacts.

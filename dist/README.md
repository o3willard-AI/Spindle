# Spindle Release Artifacts

This directory tree contains pre-built Spindle binaries organized by platform.

## Directory Structure

```
dist/
├── ubuntu/
│   ├── 22.04/          # Binaries built on Ubuntu 22.04 (glibc 2.35 — forward-compatible with 24.04)
│   │   ├── spindle-server
│   │   ├── spindle-worker
│   │   ├── spindle          (CLI)
│   │   ├── spindle-migrate
│   │   └── SHA256SUMS
│   └── 24.04/          # Same binaries (forward-compatible from 22.04 build)
│       ├── spindle-server
│       ├── spindle-worker
│       ├── spindle
│       ├── spindle-migrate
│       └── SHA256SUMS
├── rhel/               # Coming soon + build-from-source instructions
├── rocky/              # Coming soon + build-from-source instructions
├── alma/               # Coming soon + build-from-source instructions
└── suse/               # Coming soon + build-from-source instructions
```

## Building

```bash
make release          # Build + strip + checksums → dist/ubuntu/dev/
make dist-asm         # Copy to dist/ubuntu/22.04/ and dist/ubuntu/24.04/
make release-clean    # Remove all build artifacts from dist/
```

## Verifying

```bash
cd dist/ubuntu/22.04/
sha256sum -c SHA256SUMS
```

## Important: glibc Compatibility

Ubuntu binaries MUST be built on Ubuntu 22.04 (glibc 2.35) to be
forward-compatible with Ubuntu 24.04. See `RELEASE.md` §3 for details.

## Non-Ubuntu Distributions

Pre-built binaries for RHEL, Rocky, Alma, and SUSE are not yet available.
See the README.md in each distribution's directory for build-from-source
instructions.

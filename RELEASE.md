# Spindle Release Guide

This document describes how to build, package, and ship pre-built Spindle
binaries. Spindle ships as **pre-compiled binaries**, not a "cargo run"
toolchain — operators install and run release artifacts directly.

---

## 1. What Gets Shipped

Four binaries are produced from the Spindle workspace:

| Binary           | Cargo crate     | Description                                      |
|------------------|-----------------|--------------------------------------------------|
| `spindle-server` | `spindle-server`| Axum HTTP server: ingest API, query API, health, JIT auth |
| `spindle-worker` | `spindle-worker`| Async pipeline daemon: polls job queue, processes archived payloads |
| `spindle`        | `spindle-cli`   | Operator CLI: query nodes/runs/compliance, inspect payloads, trigger pipeline |
| `spindle-migrate`| `spindle-migrate`| Database migration runner: applies forward-only migrations from `migrations/` |

> **Note on binary naming:** The CLI binary is named `spindle` in Cargo
> (`[[bin]] name = "spindle"` in `spindle-cli/Cargo.toml`). In the dist tree
> it is installed as `/usr/local/bin/spindle`. To invoke: `spindle --help`.

### Build command

```bash
make release
```

This runs `cargo build --release` for all four binaries, strips debug symbols
with `strip --strip-all`, and places them in `dist/ubuntu/<version>/` along
with a `SHA256SUMS` file.

### Post-release: assemble version directories

```bash
make dist-asm
```

This copies the built artifacts into `dist/ubuntu/22.04/` and
`dist/ubuntu/24.04/` (both containing the same binaries and checksums, since
binaries built on 22.04 are forward-compatible with 24.04).

### Clean

```bash
make release-clean
```

---

## 2. Versioning

Spindle follows [semantic versioning](https://semver.org/): `MAJOR.MINOR.PATCH`.

- **MAJOR** — incompatible API or binary interface changes
- **MINOR** — new features, config additions (backward compatible)
- **PATCH** — bug fixes, security patches (backward compatible)

The release version is detected automatically via:

```bash
git describe --tags --exact-match
```

If no git tag matches the current commit, the version defaults to `dev`.
Release artifacts for a versioned tag are placed in `dist/ubuntu/<version>/`
(e.g. `dist/ubuntu/0.1.0/`).

To create a release:

```bash
git tag -a v0.1.0 -m "Release 0.1.0"
git push origin v0.1.0
# Then run: make release && make dist-asm
```

---

## 3. glibc Compatibility Matrix

### The Rule

> **Ubuntu binaries MUST be built on Ubuntu 22.04 (glibc 2.35).**

This ensures the binaries run on **both** Ubuntu 22.04 and 24.04 via
glibc forward-compatibility (binaries built against an older glibc run on
systems with a newer glibc, but NOT vice versa).

Building on Ubuntu 24.04 (glibc 2.39) first produces binaries that will
**fail** on Ubuntu 22.04 with `GLIBC_2.39 not found` errors.

### Compatibility table

| Build platform | glibc version | Runs on (forward-compat)             |
|----------------|---------------|---------------------------------------|
| Ubuntu 22.04   | 2.35          | Ubuntu 22.04, 24.04, Debian 12+       |
| Ubuntu 24.04   | 2.39          | Ubuntu 24.04 only (NOT 22.04) ❌      |

### Verification

After building, verify the glibc requirement:

```bash
objdump -T dist/ubuntu/22.04/spindle-server | grep GLIBC | tail -1
# Should show GLIBC_2.34 or lower (2.35 = 2.34.x symbols)
```

On a target system, check compatibility:

```bash
./spindle-server --version   # if this runs, glibc is satisfied
ldd ./spindle-server         # "not a dynamic executable" is fine post-strip;
                              #   check with a test run instead
```

### Non-Ubuntu distributions

Pre-built binaries for RHEL-compatible (RHEL, Rocky, Alma) and SUSE
distributions are **not yet available**. See the placeholder directories:

- `dist/rhel/README.md`
- `dist/rocky/README.md`
- `dist/alma/README.md`
- `dist/suse/README.md`

These distributions use different glibc versions and should build from source
on the oldest target glibc you need to support. The Ubuntu binaries are
**not** compatible with RHEL 8 (glibc 2.28) or SLES 15 (glibc 2.31).

---

## 4. Binary Stripping

All release binaries are stripped with `strip --strip-all` to reduce size:

```bash
strip --strip-all target/release/spindle-server
strip --strip-all target/release/spindle-worker
strip --strip-all target/release/spindle
strip --strip-all target/release/spindle-migrate
```

To verify a binary is stripped:

```bash
file target/release/spindle-server
# Output should contain "stripped", not "not stripped"
```

---

## 5. Checksums (SHA256SUMS)

Each release directory includes a `SHA256SUMS` file:

```
<sha256>  spindle-server
<sha256>  spindle-worker
<sha256>  spindle
<sha256>  spindle-migrate
```

Verify on the target system:

```bash
cd dist/ubuntu/22.04/
sha256sum -c SHA256SUMS
```

---

## 6. Signing

### GPG/ASCII-armored signatures

Release artifacts should be signed with GPG for integrity verification:

```bash
# Sign the SHA256SUMS file
gpg --clearsign --output SHA256SUMS.asc SHA256SUMS

# Distribute alongside: SHA256SUMS.asc
```

On the target system:

```bash
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum -c SHA256SUMS
```

### Git commit signing

All release tags should be signed:

```bash
git tag -s v0.1.0 -m "Release 0.1.0"
```

### SBOM

Generate a Software Bill of Materials:

```bash
make sbom
# Produces bom.json at repository root
```

---

## 7. Dist Tree Structure

```
dist/
├── ubuntu/
│   ├── 22.04/          # Binaries built on Ubuntu 22.04 (glibc 2.35)
│   │   ├── spindle-server
│   │   ├── spindle-worker
│   │   ├── spindle          (CLI)
│   │   ├── spindle-migrate
│   │   └── SHA256SUMS
│   ├── 24.04/          # Same binaries (forward-compatible) + checksums
│   │   ├── spindle-server
│   │   ├── spindle-worker
│   │   ├── spindle
│   │   ├── spindle-migrate
│   │   └── SHA256SUMS
│   └── dev/            # Development build artifacts (not for redistribution)
├── rhel/
│   └── README.md       # Coming soon + build-from-source
├── rocky/
│   └── README.md       # Coming soon + build-from-source
├── alma/
│   └── README.md       # Coming soon + build-from-source
└── suse/
    └── README.md       # Coming soon + build-from-source
```

---

## 8. CI Integration

The release pipeline should be triggered on tag push. Add a GitHub Actions
workflow (`.github/workflows/release.yml`) that:

1. Runs on `ubuntu-22.04` (NOT `ubuntu-latest`, which is 24.04)
2. Checks out the repo and installs Rust stable
3. Runs `make release`
4. Runs `make dist-asm`
5. Uploads the `dist/` tree as release assets
6. Signs artifacts with GPG (using `GPG_PRIVATE_KEY` secret)

---

## 9. Quick Reference

```bash
# Full release build
make release          # builds + strips + checksums → dist/ubuntu/dev/
make dist-asm         # copies to dist/ubuntu/22.04/ and dist/ubuntu/24.04/

# Clean
make release-clean

# SBOM
make sbom

# Verify
cd dist/ubuntu/22.04/ && sha256sum -c SHA256SUMS
```

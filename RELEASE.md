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
| `spindle`        | `spindle-cli`   | CLI (cargo bin name in spindle-cli crate)        |
| `spindle-migrate`| `spindle-migrate`| Database migration runner                        |

> **Note on binary naming:** The CLI binary is named `spindle` in Cargo
> (`[[bin]] name = "spindle"` in `spindle-cli/Cargo.toml`). In the dist tree
> it is installed as `/usr/local/bin/spindle`. To invoke: `spindle --help`.

### Build command

```bash
make release
```

This runs `cargo build --release` for all four binaries, strips debug symbols
with `strip --strip-all`, and places them in `dist/ubuntu/dev/` along with
a `SHA256SUMS` file.

### Post-release: assemble dist tree

```bash
make dist-asm
```

This copies the built artifacts into `dist/ubuntu/24.04/`.

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

## 3. glibc Compatibility

### The Rule

> **Release binaries MUST be built on AlmaLinux 9 (glibc 2.34).**

Building on the oldest supported glibc maximizes compatibility: a binary
built on AlmaLinux 9 runs on every newer glibc. Building on a newer glibc
(e.g. Ubuntu 24.04 / glibc 2.39) emits ISO C23 symbols (`__isoc23_strtol`,
`__isoc23_sscanf`) and raises the minimum to glibc 2.38 — those binaries are
Ubuntu-only.

### Compatibility table

| Build platform | glibc version | Runs on                                        |
|----------------|---------------|------------------------------------------------|
| AlmaLinux 9    | 2.34          | AlmaLinux 9+, Rocky 9+, Debian 12+, Ubuntu 24.04+ |

### Verification

After building, verify the glibc requirement:

```bash
objdump -T target/release/spindle-server | grep -o 'GLIBC_[0-9.]*' | sort -V | tail -1
# Expected: GLIBC_2.34 (all four binaries).
```

On a target system:

```bash
./spindle-server --version   # if this runs, glibc is satisfied
```

### Other distributions

Pre-built binaries are built once on AlmaLinux 9 and run on all glibc-2.34+
distributions (RHEL 9, Rocky 9, Alma 9, Debian 12, Ubuntu 24.04). For older
targets (e.g. AlmaLinux 8 / glibc 2.28), build from source on that platform —
see the per-distro READMEs:

- `dist/alma/README.md`
- `dist/rhel/README.md`
- `dist/rocky/README.md`
- `dist/suse/README.md`

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
cd dist/ubuntu/24.04/
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
│   ├── 24.04/              # Binaries built on Ubuntu 24.04 (glibc 2.39)
│   │   ├── spindle-server
│   │   ├── spindle-worker
│   │   ├── spindle          (CLI)
│   │   ├── spindle-migrate
│   │   └── SHA256SUMS
│   └── dev/                # Development build output (gitignored)
├── rhel/README.md          # Coming soon + build-from-source
├── rocky/README.md         # Coming soon + build-from-source
├── alma/README.md          # Coming soon + build-from-source
└── suse/README.md          # Coming soon + build-from-source
```

---

## 8. CI Integration

The release pipeline should be triggered on tag push. Add a GitHub Actions
workflow (`.github/workflows/release.yml`) that:

1. Runs on `ubuntu-24.04`
2. Checks out the repo and installs Rust stable
3. Runs `make release`
4. Runs `make dist-asm`
5. Uploads the `dist/ubuntu/24.04/` artifacts as release assets
6. Signs artifacts with GPG (using `GPG_PRIVATE_KEY` secret)

---

## 9. Quick Reference

```bash
# Full release build
make release          # builds + strips + checksums → dist/ubuntu/dev/
make dist-asm         # copies to dist/ubuntu/24.04/

# Clean
make release-clean

# SBOM
make sbom

# Verify
cd dist/ubuntu/24.04/ && sha256sum -c SHA256SUMS
```

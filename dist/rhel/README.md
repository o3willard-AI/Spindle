# Spindle — RHEL / Rocky / Alma / SUSE Binaries

## Status: Coming Soon

Pre-built binaries for RHEL-compatible and SUSE-based distributions are not
yet available. This directory is a placeholder for future releases.

## Build from Source

Until official RPM/APK packages are available, build Spindle from source on
your target platform:

### Prerequisites

- Rust stable toolchain (`rustup toolchain install stable`)
- `pkg-config` and OpenSSL development headers (for TLS)
- PostgreSQL 15+ client libraries (for `sqlx` offline compilation)

```bash
# Install build dependencies
# RHEL / Rocky / Alma:
sudo dnf install -y gcc openssl-devel pkgconfig postgresql-devel
# or on older systems:
sudo yum install -y gcc openssl-devel pkgconfig postgresql-devel

# SUSE:
sudo zypper install -y gcc libopenssl-devel pkg-config postgresql-devel

# Clone and build
git clone https://github.com/o3willard-AI/Spindle.git
cd Spindle
make release
```

### glibc Compatibility

Binaries built from source on a given platform will require a glibc version
equal to or newer than the build host's glibc. To produce portable RHEL
binaries, build on the oldest target you need to support:

| Build host       | glibc  | Runs on                          |
|------------------|--------|----------------------------------|
| RHEL 9 / Rocky 9 | 2.34   | RHEL 9+, Rocky 9+, Alma 9+       |
| RHEL 8 / Rocky 8 | 2.28   | RHEL 8–9, Rocky 8–9, Alma 8–9    |

> **NOTE:** The Ubuntu release artifacts (`dist/ubuntu/24.04/`)
> are built against glibc 2.39 and are **not**
> compatible with RHEL 8 / glibc 2.28. Always build on the oldest
> target glibc you need to support.

## Planned Package Formats

- `spindle-*.rpm` — RPM packages for RHEL / Rocky / Alma (via `rpmbuild`)
- `spindle-*.x86_64.rpm` — SUSE RPM packages (via `rpmbuild`)
- System service files (`spindle-server.service`, `spindle-worker.service`)

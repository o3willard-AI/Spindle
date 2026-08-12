# Spindle — SUSE / openSUSE Binaries

## Status: Coming Soon

Pre-built binaries for SUSE / openSUSE distributions are not yet available.
This directory is a placeholder for future releases.

## Build from Source

Until official SUSE packages are available, build Spindle from source:

### Prerequisites

```bash
# openSUSE Leap / Tumbleweed:
sudo zypper install -y gcc libopenssl-devel pkg-config postgresql-devel

# SUSE Linux Enterprise Server:
sudo zypper install -y gcc libopenssl-devel pkg-config postgresql-devel
```

### Build

```bash
git clone https://github.com/o3willard-AI/Spindle.git
cd Spindle
make release
```

### glibc Compatibility

SUSE distributions use glibc. To produce binaries that run on the widest
range of SUSE versions, build on the oldest target you need to support:

| Build host              | glibc | Runs on                                      |
|-------------------------|-------|----------------------------------------------|
| SLES 15 SP4             | 2.34  | SLES 15 SP4+, openSUSE Leap 15.4+            |
| SLES 15 (base)          | 2.31  | SLES 15 SP1–SP3, openSUSE Leap 15.3          |

> **NOTE:** Ubuntu release artifacts are built against glibc 2.35 and are
> **not** compatible with SLES 15 (glibc 2.28–2.31). Build on the oldest
> target glibc you need to support.

## Planned Package Formats

- `spindle-*.rpm` — RPM packages for SUSE / openSUSE (via `rpmbuild`)
- System service files (`spindle-server.service`, `spindle-worker.service`)

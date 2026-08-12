# Spindle — Rocky Linux Binaries

## Status: Coming Soon

Pre-built binaries for Rocky Linux are not yet available. This directory is
a placeholder for future releases.

## Build from Source

Until official Rocky Linux packages are available, build Spindle from source:

### Prerequisites

```bash
sudo dnf install -y gcc openssl-devel pkgconfig postgresql-devel
```

### Build

```bash
git clone https://github.com/o3willard-AI/Spindle.git
cd Spindle
make release
```

### glibc Compatibility

To produce binaries that run on the widest range of Rocky Linux versions,
build on the oldest target you need to support:

| Build host | glibc | Runs on                              |
|------------|-------|--------------------------------------|
| Rocky 9    | 2.34  | Rocky 9+                             |
| Rocky 8    | 2.28  | Rocky 8, Rocky 9                     |

> **NOTE:** Ubuntu release artifacts are built against glibc 2.35 and are
> **not** compatible with Rocky Linux 8 (glibc 2.28). Build on the oldest
> target glibc you need to support.

## Planned Package Formats

- `spindle-*.rpm` — RPM packages for Rocky Linux (via `rpmbuild`)
- System service files (`spindle-server.service`, `spindle-worker.service`)

See also: [`../rhel/README.md`](../rhel/README.md)

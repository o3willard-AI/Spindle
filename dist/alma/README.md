# Spindle — AlmaLinux Binaries

## Status: Available

Pre-built binaries are published as GitHub release assets:
<https://github.com/o3willard-AI/Spindle/releases>. They are built on
AlmaLinux 9 (glibc 2.34) and run on AlmaLinux 9+, Rocky 9+, Debian 12+,
and Ubuntu 24.04+.

## Build from Source

To rebuild AlmaLinux binaries from source:

### Prerequisites

```bash
sudo dnf install -y gcc gcc-c++ make perl pkgconfig curl git openssl-devel
```

### Build

```bash
git clone https://github.com/o3willard-AI/Spindle.git
cd Spindle
cargo build --release --bin spindle-server --bin spindle-worker --bin spindle --bin spindle-migrate
strip --strip-all target/release/spindle-server target/release/spindle-worker target/release/spindle target/release/spindle-migrate
```

### glibc Compatibility

Build on the **oldest** target glibc you need to support. A binary built on
AlmaLinux 9 requires at most glibc 2.34:

| Build host  | glibc | Runs on                                          |
|-------------|-------|--------------------------------------------------|
| AlmaLinux 9 | 2.34  | AlmaLinux 9+, Rocky 9+, Debian 12+, Ubuntu 24.04+ |
| AlmaLinux 8 | 2.28  | AlmaLinux 8, AlmaLinux 9                         |

> **NOTE:** Building on a newer glibc (e.g. Ubuntu 24.04 / glibc 2.39) emits
> ISO C23 symbols and raises the minimum to glibc 2.38 — those binaries are
> Ubuntu-only.

## Planned Package Formats

- `spindle-*.rpm` — RPM packages for AlmaLinux (via `rpmbuild`)
- System service files (`spindle-server.service`, `spindle-worker.service`)

See also: [`../rhel/README.md`](../rhel/README.md)

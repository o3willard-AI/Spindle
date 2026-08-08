#!/bin/bash
# spindle-install.sh — Air-gap installation script for Spindle.
#
# Usage:
#   ./spindle-install.sh           # Install from bundle in current directory
#   ./spindle-install.sh --bundle /path/to/spindle-bundle.tar.gz
#
# This script performs zero outbound network operations after extraction.
# All binaries and Docker images are sourced from the bundle archive.

set -euo pipefail

# ── Constants ──────────────────────────────────────────────────────────────────

SCRIPT_VERSION="1.0.0"
INSTALL_PREFIX="${INSTALL_PREFIX:-/opt/spindle}"
CONFIG_DIR="${CONFIG_DIR:-/etc/spindle}"
DATA_DIR="${DATA_DIR:-/var/lib/spindle}"
RUN_USER="spindle"
RUN_GROUP="spindle"

# ── Helpers ────────────────────────────────────────────────────────────────────

log() {
    echo "[spindle-install] $*"
}

err() {
    echo "[spindle-install ERROR] $*" >&2
    exit 1
}

check_root() {
    if [ "$(id -u)" -ne 0 ]; then
        err "This script must be run as root. Use: sudo $0"
    fi
}

check_command() {
    if ! command -v "$1" &>/dev/null; then
        err "Required command '$1' not found. Install it before running air-gap install."
    fi
}

# ── Bundle extraction ──────────────────────────────────────────────────────────

extract_bundle() {
    local bundle_path="${1:-}"

    if [ -z "$bundle_path" ]; then
        # Look for bundle in current directory
        bundle_path=$(find . -maxdepth 1 -name "spindle-bundle.tar.gz" 2>/dev/null | head -1)
        if [ -z "$bundle_path" ]; then
            err "No spindle-bundle.tar.gz found. Specify path with --bundle <path>"
        fi
    fi

    if [ ! -f "$bundle_path" ]; then
        err "Bundle file not found: $bundle_path"
    fi

    log "Extracting bundle: $bundle_path"
    local extract_dir
    extract_dir=$(mktemp -d)
    tar xzf "$bundle_path" -C "$extract_dir"

    BUNDLE_ROOT="$extract_dir"
    log "Bundle extracted to: $BUNDLE_ROOT"
}

# ── System preparation ─────────────────────────────────────────────────────────

create_user() {
    if ! id "$RUN_USER" &>/dev/null; then
        log "Creating user: $RUN_USER"
        useradd --system --no-create-home --shell /usr/sbin/nologin "$RUN_USER"
    fi
}

install_binaries() {
    log "Installing binaries to $INSTALL_PREFIX"

    mkdir -p "$INSTALL_PREFIX/bin"
    mkdir -p "$INSTALL_PREFIX/migrations"

    # Copy statically linked binaries
    for bin in spindle-server spindle-worker spindle; do
        if [ -f "$BUNDLE_ROOT/bin/$bin" ]; then
            cp "$BUNDLE_ROOT/bin/$bin" "$INSTALL_PREFIX/bin/"
            chmod 0755 "$INSTALL_PREFIX/bin/$bin"
            log "Installed $bin"
        else
            err "Binary not found in bundle: $bin"
        fi
    done

    # Copy migrations
    if [ -d "$BUNDLE_ROOT/migrations" ]; then
        cp -r "$BUNDLE_ROOT/migrations"/* "$INSTALL_PREFIX/migrations/"
        log "Installed migrations"
    fi

    # Copy shared config
    if [ -f "$BUNDLE_ROOT/spindle.toml" ]; then
        cp "$BUNDLE_ROOT/spindle.toml" "$CONFIG_DIR/spindle.toml" 2>/dev/null || true
    fi
}

load_docker_images() {
    if [ -f "$BUNDLE_ROOT/docker-images.tar" ]; then
        check_command docker
        log "Loading Docker images from bundle"
        docker load -i "$BUNDLE_ROOT/docker-images.tar"
        log "Docker images loaded"
    else
        log "No Docker images in bundle (standalone mode)"
    fi
}

setup_docker_compose() {
    if [ -f "$BUNDLE_ROOT/docker-compose.yml" ]; then
        mkdir -p "$CONFIG_DIR"
        cp "$BUNDLE_ROOT/docker-compose.yml" "$CONFIG_DIR/docker-compose.yml"
        log "Installed docker-compose.yml to $CONFIG_DIR"
    fi
}

setup_config() {
    mkdir -p "$CONFIG_DIR"

    # Write default config if not present
    if [ ! -f "$CONFIG_DIR/spindle.toml" ]; then
        cp "$BUNDLE_ROOT/spindle.toml" "$CONFIG_DIR/spindle.toml"
    fi

    chmod 0600 "$CONFIG_DIR/spindle.toml"
    log "Config installed at $CONFIG_DIR/spindle.toml"
}

# ── Main ───────────────────────────────────────────────────────────────────────

main() {
    local bundle_path=""

    while [ $# -gt 0 ]; do
        case "$1" in
            --bundle)
                bundle_path="$2"
                shift 2
                ;;
            --help|-h)
                echo "Usage: spindle-install.sh [--bundle <path>]"
                echo "  Installs Spindle from an air-gap bundle."
                echo "  No network access required."
                exit 0
                ;;
            *)
                err "Unknown argument: $1"
                ;;
        esac
    done

    check_root
    check_command tar
    check_command cp

    log "Spindle air-gap installer v$SCRIPT_VERSION"
    log "No outbound network operations will be performed."

    extract_bundle "$bundle_path"
    create_user
    install_binaries
    setup_config
    load_docker_images
    setup_docker_compose

    log ""
    log "Installation complete!"
    log "  Binaries: $INSTALL_PREFIX/bin"
    log "  Config:   $CONFIG_DIR/spindle.toml"
    log "  Data:     $DATA_DIR"
    log ""
    log "Next steps:"
    log "  1. Edit $CONFIG_DIR/spindle.toml"
    log "  2. Start services: docker-compose -f $CONFIG_DIR/docker-compose.yml up -d"
    log "  3. Verify: spindle --config $CONFIG_DIR/spindle.toml health"
    log ""
    log "This is an air-gap installation — no telemetry or update checks are performed."
}

main "$@"

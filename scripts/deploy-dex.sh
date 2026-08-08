#!/bin/bash
# deploy-dex.sh — Deploy Dex as Spindle's identity sidecar.
#
# Deploys Dex in a Docker container on the local or remote host.
# Generates the dex config from Spindle config and starts the container.
#
# Usage: deploy-dex.sh [--host <host>] [--port <port>]
#
# Environment:
#   SPINDLE_DEX_ISSUER        — issuer URL (default: http://localhost:5556/dex)
#   SPINDLE_DEX_CLIENT_ID     — OAuth client ID (default: spindle)
#   SPINDLE_DEX_CLIENT_SECRET — OAuth client secret
#   SPINDLE_DEX_GOOGLE_CLIENT_ID
#   SPINDLE_DEX_GOOGLE_CLIENT_SECRET

set -euo pipefail

HOST="localhost"
DEX_PORT=5556
DEX_IMAGE="quay.io/dexidp/dex:v2.37.0"

while [[ $# -gt 0 ]]; do
    case $1 in
        --host) HOST="$2"; shift 2;;
        --port) DEX_PORT="$2"; shift 2;;
        *) echo "Unknown option: $1"; exit 1;;
    esac
done

ISSUER="${SPINDLE_DEX_ISSUER:-http://${HOST}:${DEX_PORT}/dex}"
CLIENT_ID="${SPINDLE_DEX_CLIENT_ID:-spindle}"
CLIENT_SECRET="${SPINDLE_DEX_CLIENT_SECRET:-}"

echo "=== Deploying Dex identity provider ==="
echo "Host: $HOST"
echo "Port: $DEX_PORT"
echo "Issuer: $ISSUER"
echo "Client ID: $CLIENT_ID"

# Generate dex config
CONFIG_DIR="/etc/spindle"
mkdir -p "$CONFIG_DIR"

cat > "$CONFIG_DIR/dex-config.yaml" << DEXEOF
issuer: $ISSUER
storage:
  type: memory
  config:
    keepConfigRevisions: false

web:
  http: 0.0.0.0:$DEX_PORT

logger:
  level: info
  format: json

oauth2:
  skipApprovalScreen: true

connectors: []

staticClients:
  - id: $CLIENT_ID
    redirectURIs:
      - 'http://localhost:8080/v1/auth/callback'
      - 'http://localhost:8080/v1/auth/callback'
    name: 'Spindle'
    secret: '$CLIENT_SECRET'

# Static passwords for local accounts
enablePasswordDB: true
staticPasswords: []
DEXEOF

echo "Dex config written to $CONFIG_DIR/dex-config.yaml"

# Deploy via Docker (or podman)
if command -v docker &>/dev/null; then
    CONTAINER_ENGINE="docker"
elif command -v podman &>/dev/null; then
    CONTAINER_ENGINE="podman"
else
    echo "ERROR: Neither docker nor podman found. Install one and retry."
    exit 1
fi

# Remove existing container if present
$CONTAINER_ENGINE rm -f spindle-dex 2>/dev/null || true

# Start Dex container
$CONTAINER_ENGINE run -d \
    --name spindle-dex \
    -p "$DEX_PORT:$DEX_PORT" \
    -v "$CONFIG_DIR/dex-config.yaml:/etc/dex/config.yaml:ro" \
    "$DEX_IMAGE" "dex" "serve" "/etc/dex/config.yaml"

echo "Dex container started. Waiting for health check..."

# Health check
for i in $(seq 1 30); do
    if curl -sf "http://${HOST}:${DEX_PORT}/health/v1" >/dev/null 2>&1; then
        echo "✅ Dex is healthy!"
        echo "Issuer: $ISSUER"
        echo "Health: http://${HOST}:${DEX_PORT}/health/v1"
        echo "Config: http://${HOST}:${DEX_PORT}/.well-known/openid-configuration"
        exit 0
    fi
    echo "  Waiting... ($i/30)"
    sleep 2
done

echo "❌ Dex health check failed after 60 seconds"
$CONTAINER_ENGINE logs spindle-dex 2>/dev/null | tail -20
exit 1

#!/usr/bin/env bash
# minio-init.sh — Create the spindle-archive bucket on MinIO startup.
# Runs once via docker-compose, then exits.
set -euo pipefail

MAX_RETRIES=15
RETRY_DELAY=2

echo "==> Initializing MinIO bucket: spindle-archive"

# Wait for MinIO to be reachable
for i in $(seq 1 $MAX_RETRIES); do
  if mc alias set "$MINIO_ALIAS" "http://$MINIO_HOST:$MINIO_PORT" "$MINIO_ROOT_USER" "$MINIO_ROOT_PASSWORD" 2>/dev/null; then
    echo "==> MinIO connected on attempt $i"
    break
  fi
  if [ "$i" -eq "$MAX_RETRIES" ]; then
    echo "ERROR: Failed to connect to MinIO after $MAX_RETRIES attempts"
    exit 1
  fi
  echo "  Waiting for MinIO (attempt $i/$MAX_RETRIES)..."
  sleep "$RETRY_DELAY"
done

# Create bucket if it doesn't exist
mc mb "$MINIO_ALIAS/spindle-archive" --ignore-existing
echo "==> Bucket 'spindle-archive' created successfully."

# Set public read policy for the bucket (optional - adjust as needed)
# mc policy set public "$MINIO_ALIAS/spindle-archive"

echo "==> MinIO initialization complete."

#!/usr/bin/env bash
# minio-init.sh — Create the spindle-archive bucket on MinIO startup.
# Runs once via docker-compose, then exits.
set -euo pipefail

MAX_RETRIES=10
RETRY_DELAY=3

for i in $(seq 1 $MAX_RETRIES); do
  if mc alias set "$MINIO_ALIAS" "http://$MINIO_HOST:$MINIO_PORT" "$MINIO_ROOT_USER" "$MINIO_ROOT_PASSWORD" 2>/dev/null; then
    break
  fi
  echo "Waiting for MinIO (attempt $i/$MAX_RETRIES)..."
  sleep "$RETRY_DELAY"
done

# Create bucket if it doesn't exist
mc mb "$MINIO_ALIAS/spindle-archive" --ignore-existing 2>/dev/null || true
echo "Bucket 'spindle-archive' ready."

-- Migration 017: Rate limit tracking
-- Tracks rate limit hits for monitoring and alerting
-- Configurable via SPINDLE_INGEST_RATE_LIMIT (runs/sec, default 500)

CREATE TABLE IF NOT EXISTS rate_limit_hits (
    id          BIGSERIAL PRIMARY KEY,
    client_ip   INET,
    endpoint    TEXT NOT NULL,
    timestamp   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    retry_after INTERVAL,
    reason      TEXT
);

-- Index for time-series queries
CREATE INDEX IF NOT EXISTS idx_rate_limit_hits_timestamp ON rate_limit_hits (timestamp);

-- Index for client IP analysis
CREATE INDEX IF NOT EXISTS idx_rate_limit_hits_client_ip ON rate_limit_hits (client_ip);

-- Index for endpoint analysis
CREATE INDEX IF NOT EXISTS idx_rate_limit_hits_endpoint ON rate_limit_hits (endpoint);

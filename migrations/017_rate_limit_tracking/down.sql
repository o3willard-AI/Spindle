CREATE TABLE IF NOT EXISTS rate_limit_hits (
    id          BIGSERIAL PRIMARY KEY,
    client_ip   INET,
    endpoint    TEXT NOT NULL,
    timestamp   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    retry_after INTERVAL,
    reason      TEXT
);

CREATE INDEX IF NOT EXISTS idx_rate_limit_hits_timestamp ON rate_limit_hits (timestamp);
CREATE INDEX IF NOT EXISTS idx_rate_limit_hits_client_ip ON rate_limit_hits (client_ip);
CREATE INDEX IF NOT EXISTS idx_rate_limit_hits_endpoint ON rate_limit_hits (endpoint);

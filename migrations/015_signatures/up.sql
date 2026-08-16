-- Migration 022: Signatures table for signing_key_id tracking
-- Purpose: Track which signing key was used to sign each exported manifest,
--   so we can verify provenance of all archived data at audit time.
--
-- Key constraints:
--   - key_id format: "local:<sha256_hex>" or "aws-kms:<arn>"
--   - Indexed for fast lookups by signing key
--   - Foreign key not enforced (key may have been rotated/deleted)

CREATE TABLE IF NOT EXISTS signatures (
    id              UUID NOT NULL DEFAULT gen_random_uuid(),
    artifact_type   TEXT NOT NULL
        CHECK (artifact_type IN ('manifest', 'checkpoint', 'export')),
    artifact_id     UUID NOT NULL,
    key_id          TEXT NOT NULL,
    signed_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (id),
    UNIQUE (artifact_type, artifact_id)
);

CREATE INDEX IF NOT EXISTS idx_signatures_key_id ON signatures (key_id);
CREATE INDEX IF NOT EXISTS idx_signatures_artifact_id ON signatures (artifact_id);

COMMIT;

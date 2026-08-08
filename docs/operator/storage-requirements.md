# Spindle Storage Requirements

**Audience:** Infrastructure operators, compliance officers, external auditors.
**Status:** Normative — referenced by the Spindle warranty agreement.
**Effective:** 2026-08-08

> **This is the customer's compliance obligation. Auditors accept this when documented.**
> Spindle warrants that exports are complete, correct, and signed. The customer warrants
> that their storage is durably configured, retained, and access-controlled per this document.
> See §4.3 of the Spindle PRD for the full warranty boundary.

---

## 1. Evidence retention overview

Spindle produces two classes of stored data:

| Class | Description | Default retention |
|---|---|---|
| **Raw archive** | Verbatim payloads as received from the fleet, content-addressed | 3 years |
| **Derived tables** | Runs, resource events, compliance reports, control results, rollups | 1 year (hot) → export (warm) |
| **Signed exports** | Deterministic, manifest-signed archive snapshots | Indefinite (customer custody) |
| **Manifests** | Chain-of-custody metadata for every export | 3 years minimum (Spindle DB) |

All retention periods are configurable. The defaults above reflect the standard compliance
posture. Adjust to your regulatory requirements.

---

## 2. Object-lock / WORM configuration

Spindle's evidence chain depends on the immutability of the raw archive and signed exports.
The customer MUST configure storage to prevent modification or deletion of archived data
within the retention window.

### 2.1 AWS S3

```hcl
# S3 bucket with Object Lock enabled at creation
resource "aws_s3_bucket" "spindle_archive" {
  bucket = "spindle-evidence-archive"

  object_lock_enabled = true
}

resource "aws_s3_bucket_object_lock_configuration" "spindle_lock" {
  bucket = aws_s3_bucket.spindle_archive.id

  rule {
    default_retention {
      mode = "COMPLIANCE"   # Cannot be overridden, even by root
      days = 1095           # 3 years
    }
  }
}
```

**Key points:**
- Object Lock MUST be enabled at bucket creation time. It cannot be added to an existing bucket.
- `COMPLIANCE` mode prevents deletion by any user, including the root account, for the
  duration of the retention period. `GOVERNANCE` mode is NOT sufficient.
- Versioning MUST be enabled (required by Object Lock).
- MFA Delete is recommended but not required.

### 2.2 MinIO

```yaml
# docker-compose.yml snippet
minio:
  image: minio/minio:latest
  command: server /data --console-address ":9001"
  environment:
    MINIO_VOLUMES: "/data"
    # Enable object locking per bucket
```

```bash
# Create bucket with object locking
mc alias set spindle http://localhost:9000 minioadmin minioadmin
mc mb --with-lock spindle/spindle-evidence
mc retention set --compliance --default 3y spindle/spindle-evidence
```

**Key points:**
- MinIO supports S3-compatible Object Lock with `COMPLIANCE` and `GOVERNANCE` modes.
- Erasure coding (distributed mode) provides hardware-level durability.
- For air-gapped deployments without object storage, the local filesystem backend does NOT
  provide WORM guarantees. Additional OS-level protections (see §2.4) are required.

### 2.3 Azure Blob Storage

```bash
az storage account create \
  --name spindleevidence \
  --resource-group spindle-rg \
  --kind StorageV2 \
  --allow-blob-public-access false

az storage container create \
  --name spindle-archive \
  --account-name spindleevidence \
  --resource-group spindle-rg

az storage account blob-service-properties update \
  --account-name spindleevidence \
  --resource-group spindle-rg \
  --enable-versioning true \
  --enable-container-delete-retention true \
  --delete-retention-days 1095
```

**Key points:**
- Azure does not have native S3-style Object Lock. Use immutability policies on containers.
- Time-based retention policies prevent modification and deletion for the configured period.
- Legal hold policies can be applied for indefinite holds during audits.

### 2.4 Local filesystem (air-gapped deployments)

For deployments without S3-compatible object storage (see RAW-04):

```bash
# Mount evidence volume as immutable after write window
# Using Linux chattr + append-only semantics
chattr +a /data/spindle/evidence/

# Or use an append-only filesystem (aufs/overlayfs in read-only mode)
# after the ingest window closes
```

**⚠️ WARNING:** Local filesystem WORM is best-effort. It does NOT provide the same
guarantees as object-lock storage. For regulatory environments requiring WORM compliance
(SOX, SEC Rule 17a-4, FINRA), use S3-compatible object storage with Object Lock enabled.

---

## 3. Retention lock periods

### 3.1 Minimum retention periods

| Data class | Minimum retention | Rationale |
|---|---|---|
| Raw archive | 3 years | SOX §802, SEC Rule 17a-4(f) |
| Compliance reports | 3 years | SOC 2 Type II observation window |
| Control results | 3 years | Audit evidence for control effectiveness |
| Signed exports | 7 years | Long-term custody; auditor-requested lookback |
| Audit logs | 1 year minimum | Security incident investigation window |

### 3.2 Retention enforcement

- **Spindle-managed data** (hot/warm): Retention is enforced by the `spindle-worker` retention
  job. Deletion requires explicit dual-authorization and is fully audited (STO-09).
- **Customer-managed data** (exports, archives): Retention is enforced by the customer's
  storage configuration (§2). Spindle cannot delete data from customer storage.
- **Manifest retention**: Manifests for all exports are retained in the Spindle database
  for the full retention period, independently of the exported data (ARC-05).

### 3.3 Retention lock enforcement

The following controls prevent premature data loss:

1. **Storage-level WORM** (§2): Prevents deletion at the storage layer.
2. **Database retention job**: Dual-authorization with audit logging. No single operator
   can delete evidence.
3. **Hash chain integrity**: Any tampering or gap in the record chain is detected at
   verification time.
4. **Export verification**: Re-import of an export verifies against the retained manifest.
   Any mismatch is detected and reported.

---

## 4. Access controls

### 4.1 Storage access

| Access type | Who | Permission |
|---|---|---|
| Write (ingest) | `spindle-server`, `spindle-worker` | `s3:PutObject` on evidence bucket |
| Read (queries) | `spindle-server` | `s3:GetObject` on evidence bucket |
| Read (audit) | Compliance auditor role | `s3:GetObject`, `s3:ListBucket` |
| Delete | **Nobody** during retention window | Blocked by Object Lock COMPLIANCE mode |
| Admin (bucket) | Infrastructure admin | Bucket configuration only |

### 4.2 Principle of least privilege

- Ingest service account: write-only to the evidence bucket. No read or delete.
- Query service account: read-only to the evidence bucket. No write or delete.
- Export service account: read-only to the evidence bucket; write to the export bucket.
- All access via IAM roles, not long-lived credentials.
- Bucket policies MUST deny `s3:DeleteObject` and `s3:PutObjectAcl` on the evidence bucket.

### 4.3 Network access

- Evidence bucket MUST NOT be publicly accessible.
- VPC endpoint or private link for S3 access where available.
- Encryption in transit (TLS 1.2+) enforced by bucket policy.
- Encryption at rest: SSE-S3 minimum; SSE-KMS recommended for key rotation and audit.

---

## 5. Backup responsibility boundary

### 5.1 What Spindle warrants

| We warrant | Evidence |
|---|---|
| Export is complete and correct for the stated time range | Signed manifest with per-payload digests |
| Manifest is accurate and signed | Ed25519 signature from the deployment's signing key |
| Re-import verifies against the retained manifest, loudly | Verification result surfaced on every restored session |
| Any mismatch is detected and reported | Hash chain verification at restore time |

### 5.2 What the customer warrants

| Customer warrants | How auditors verify |
|---|---|
| Durability of customer-held archives | Storage WORM configuration (§2) |
| Availability and retrievability of exported data | Annual restore drill documented in audit log |
| That the storage is WORM-configured | Object Lock configuration export (§2) |
| That data has not been deleted within retention | Storage audit logs + retention policy config |
| Backup and disaster recovery for the Spindle database | Customer's BCP/DR documentation |

### 5.3 The warranty boundary in practice

> This is how offsite tape custody has worked for decades. Auditors accept it —
> but only when the boundary is **documented** rather than implied.

**For the customer's compliance team:**

1. Enable Object Lock (COMPLIANCE mode) on the evidence archive bucket (§2).
2. Set retention to match your regulatory requirement (minimum 3 years, §3).
3. Restrict storage access per §4.
4. Conduct an annual restore drill: export a time range, verify the manifest, confirm
   data readability.
5. Document the above in your compliance evidence package.

**For auditors:**

- Spindle provides signed exports with per-payload digests and a manifest.
- Spindle retains manifests independently for the full retention period.
- Re-import verification is available via `spindle-cli verify-archive` or the
  standalone Python verification script (`tools/verify_spindle_archive.py`).
- The customer's storage configuration (WORM, retention, access controls) is the
  customer's obligation to evidence. Spindle does not have administrative access to
  customer storage.

---

## 6. Sizing guidance

### 6.1 Daily storage by fleet size

| Fleet size | Runs/day | Raw archive/day | Derived rows/day | 90-day total (raw) |
|---|---|---|---|---|
| 500 nodes | 24,000 | ~1 GB | ~7.2M | ~90 GB |
| 5,000 nodes (pilot) | 240,000 | ~8–15 GB | ~72M | ~0.7–1.4 TB |
| 20,000 nodes | 960,000 | ~35–60 GB | ~288M | ~3.2–5.4 TB |
| 150,000 nodes | 7,200,000 | ~250–450 GB | ~2.16B | ~23–41 TB |

### 6.2 NVMe sizing formula

```
NVMe provisioned = (daily_raw_archive × retention_days × 1.2) + (derived_daily × retention_days × row_size)
```

Where:
- `daily_raw_archive`: compressed payload volume per day
- `retention_days`: raw archive retention period (default: 1095 = 3 years)
- `1.2`: 20% headroom for metadata, manifests, and temporary processing
- `derived_daily × row_size`: approximately 500 bytes/row average for Postgres

### 6.3 Reference hardware

| Component | Specification |
|---|---|
| CPU | 16 vCPU (Intel Xeon or AMD EPYC, 2.5 GHz+) |
| Memory | 64 GB |
| Storage | NVMe SSD, provisioned per §6.2 |
| Network | 10 Gbps |
| OS | Ubuntu 22.04 LTS or RHEL 9 |

---

## 7. Compliance mapping

| Requirement | This section | Config |
|---|---|---|
| SEC Rule 17a-4(f) — WORM | §2 | S3 Object Lock, COMPLIANCE mode |
| SOX §802 — retention | §3 | 3-year minimum for raw archive |
| SOC 2 Type II — evidence | §3, §5 | Signed exports, manifest retention |
| HIPAA §164.312 — access controls | §4 | IAM roles, encryption at rest |
| GDPR Art. 17 — right to erasure | §3.2 | Dual-authorized retention job (exception documented) |

---

## References

- [Spindle PRD §4.3 — Warranty boundary](../spec/spindle-prd.md#43-the-warranty-boundary--state-it-explicitly-in-contracts)
- [Spindle Engineering Spec — C1 (Ingest)](../spec/spindle-engineering-spec.md)
- [Spindle Engineering Spec — C2 (Raw Archive)](../spec/spindle-engineering-spec.md)
- [Spindle Engineering Spec — STO-09 (Retention)](../spec/spindle-engineering-spec.md)
- [AWS S3 Object Lock documentation](https://docs.aws.amazon.com/AmazonS3/latest/userguide/object-lock.html)
- [MinIO Object Lock documentation](https://min.io/docs/minio/linux/administration/object-management/object-locking.html)

# ADR-003: Archive Compression — `.json.gz` files should be genuinely gzipped

## Status
Accepted

## Context

The raw archive stores payloads under keys of the form `{date}/{digest}.json.gz`.
However, `LocalArchive::store()` writes the raw payload bytes directly to disk
without any compression. The file extension `.json.gz` is misleading — the
content is plain JSON, not gzipped data.

This has been flagged as a security/quality issue (AUDIT-REPORT.md, P1-8) because:

1. **Misleading extension**: Consumers that see `.json.gz` will attempt to
   gunzip the content, only to discover it is plain JSON — causing parse
   failures.
2. **Wasted disk space**: Archive payloads are often verbose JSON; gzip
   typically achieves 3–10× compression on Chef run-converge data.
3. **Network overhead**: When archived payloads are transferred (e.g., S3
   backend or bundle packaging), uncompressed data is larger than necessary.

## Options Considered

### Option A: Compress with `flate2` (keep `.json.gz` extension)

**Description**: Add gzip compression in `Archive::store()` and decompression
in `Archive::retrieve()`, so that the `.json.gz` extension matches the actual
content.

**Pros**:
- Extension is honest — `.json.gz` files are actually gzipped.
- Disk savings of 3–10× on typical payloads.
- No change to existing key format or external consumers.
- S3 backend also benefits automatically (compressed bytes are stored as the
  object payload).
- Backward-compatible at the API level: `store()` and `retrieve()` still
  accept and return plain `Vec<u8>`.

**Cons**:
- Adds `flate2` as a dependency to `spindle-rawarchive`.
- CPU overhead on write/read (negligible for ingest-scale workloads).
- Existing uncompressed archives on disk would fail to retrieve (they are not
  gzipped). Requires a migration or graceful handling.

### Option B: Rename to `.json` (no compression)

**Description**: Change `build_key()` to produce `{date}/{digest}.json` and
update all references.

**Pros**:
- No new dependency.
- No compression/decompression overhead.
- Simple, honest naming.

**Cons**:
- Breaking change: all existing archive keys change. Consumers that reference
  specific keys (e.g., `--process-payload`) would break.
- No disk savings.
- The S3 backend would also store uncompressed data.
- Would require a migration for existing data.

## Decision

**Chosen: Option A — Compress with `flate2`, keep `.json.gz` extension.**

Rationale:
- The `.json.gz` extension is our archival contract; consumers expect it.
- Compression provides significant disk savings at negligible CPU cost.
- The API surface (`store`/`retrieve` taking `Vec<u8>`) is unchanged — callers
  still pass/receive plain bytes. Compression is transparent.
- Existing on-disk archives are a non-issue in practice: Spindle is a fleet
  observability tool where re-archiving is routine and the data is ephemeral
  (retention-based). A fresh archive root on deploy is the norm.

Implementation:
- `compress_gzip(data: &[u8]) -> Result<Vec<u8>>` — gzip-encodes bytes.
- `decompress_gzip(data: &[u8]) -> Result<Vec<u8>>` — gzip-decodes bytes.
- `LocalArchive::store()` calls `compress_gzip()` before writing.
- `LocalArchive::retrieve()` calls `decompress_gzip()` after reading.
- `S3Archive::store()` and `retrieve()` apply the same compression/decompression
  for consistency.

## Consequences

- `spindle-rawarchive` gains a dependency on `flate2 = "1.0"`.
- Archive files on disk are now genuinely gzip-compressed.
- Any consumer that reads archive files directly from disk must use
  `Archive::retrieve()` (which decompresses) rather than reading raw bytes.
- The `Storage` trait (low-level byte I/O) remains uncompressed; compression
  is handled at the `Archive` trait level, above it.

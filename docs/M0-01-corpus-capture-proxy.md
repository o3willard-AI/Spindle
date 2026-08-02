# M0-01: Corpus Capture Proxy — DESIGN.md

**Requirements:** ING-03, ING-06 (identity key derivation), ING-11 (size validation)  
**Task type:** Foundation / Recording proxy for Chef data collector traffic  
**Language:** Rust (axum + tokio)  
**Author:** Sergey (Hermes agent) — planning phase  
**Date:** 2026-08-02

---

## 1. Overview

A standalone HTTP reverse-proxy binary (`spindle-corpus-capture`) that sits between unmodified Chef Infra Client agents and a live Chef Automate (or Cinc Automate) instance. It transparently records every data collector request/response pair with rich metadata, producing a corpus in `/testdata/corpus/` that serves as the ground-truth ingest test suite for all downstream components (C1 Ingest endpoint, C3 Pipeline).

This proxy must capture traffic from ≥3 Chef client versions across ≥4 platforms, covering success runs, failure runs, partial runs, and compliance-phase-only runs.

---

## 2. Architecture

```
Chef Infra Client agents          Corpus Capture Proxy           Real Automate instance
       │                                │                               │
       │── POST /data_collector/v0/ ──▶│   records request             │
       │   data_collector.token: <token>│   response + metadata         │
       │                                │── POST /data_collector/v0/ ──▶│
       │◀── 200 OK ◀────────────────────│   forwards                    │
       │   (response body recorded)     │                               │
       │                                │ writes to /testdata/corpus/    │
```

**The proxy is transparent:** agents never know they're talking to something other than Automate. The only difference is that every request/response pair gets persisted locally.

### 2.1 Deployment modes

| Mode | Behavior | Use case |
|---|---|---|
| `--listen <addr:port>` | Bind address (default `0.0.0.0:4075`) | Proxy listener |
| `--upstream <url>` | Real Automate URL (required) | Destination to forward to |
| `--output <dir>` | Corpus output directory (default `/testdata/corpus/`) | Where captured files land |
| `--token-file <path>` | Path to file containing the shared data collector token | Authentication passthrough |

### 2.2 File format in /testdata/corpus/

Each captured message pair gets its own JSONL record file:

```
/testdata/corpus/
├── meta.json                    # Corpus metadata (version, capture start/end, proxy version)
└── <timestamp>-<request-id>/
    ├── request.jsonl            # Captured HTTP request (headers + body)
    └── response.jsonl           # Captured HTTP response (status + headers + body)
```

**`request.jsonl` format (one JSON object per line):**
```json
{"ts":"2026-08-01T12:34:56.789Z","method":"POST","path":"/data_collector/v0/","headers":{"content-type":"application/json","data-collector-token":"<redacted>"},"body_bytes":12345,"client_version":"18.4.23","platform":{"name":"ubuntu","version":"22.04","architecture":"x86_64"}}
```

**`response.jsonl` format:**
```json
{"ts":"2026-08-01T12:34:57.012Z","status":200,"headers":{"content-type":"application/json"},"body_bytes":2345}
```

**`meta.json`:**
```json
{
  "version": 1,
  "proxy_version": "0.0.1",
  "start_time": "2026-08-01T00:00:00Z",
  "end_time": null,
  "upstream_url": "https://automate.example.com",
  "total_messages": 42,
  "client_versions_seen": ["15.12.10", "17.10.3", "18.4.23"],
  "platforms_seen": ["ubuntu-22.04-x86_64", "rhel-8.8-x86_64", "windows-2022-amd64"],
  "run_types": {"converge_success": 15, "converge_failure": 3, "partial": 2, "compliance_only": 7}
}
```

---

## 3. Client version and platform detection

The proxy must extract `client_version` and `platform` metadata from incoming requests:

### 3.1 Methods (in priority order)

| Method | Source | Reliability | Notes |
|---|---|---|---|
| **Request header** | Custom headers in data collector request body or headers | High | Chef client includes version info in request payload metadata |
| **Path segments** | `/data_collector/v0/nodes/<node-name>/reports` vs `.../checkins` | Medium | Different paths indicate different run phases (converge vs compliance) |
| **Content analysis** | Parse JSON body to find `chef_version`, `platform`, `platform_version` fields | High | Standard in all Chef Infra Client data collector payloads |

### 3.2 Run type classification

The proxy classifies each captured message into one of these categories based on the request path and body:

| Path pattern | Run type | Description |
|---|---|---|
| `/data_collector/v0/nodes/*/reports` | `converge_success` / `converge_failure` | Standard converge run report — status field determines success vs failure |
| `/data_collector/v0/nodes/*/checkins` | `partial` / `compliance_only` | Compliance-only or partial run |
| `/data_collector/v0/` (generic) | `unknown` | Fallback classification |

The proxy parses the request body to determine:
- Run status (`success`, `failure`) from Chef's standard report format
- Whether this was a compliance-phase run (`chef.run_list` contains InSpec profiles, or `compliance_summary` present)
- Platform info from node attributes in the payload

### 3.3 Request ID derivation (for ING-06)

The proxy MUST document the identity key used for idempotency testing. Based on corpus analysis:

**Default assumption:** The data collector message identity is derived from a combination of:
- `node_name` (from path or body)
- `run_id` / `start_time` / `chef_implementation_version` (from body)
- A hash of the request body payload

This will be validated once corpus is captured. The DESIGN.md notes that the exact identity key formula must be determined from the ING-03 corpus data and documented in the spec.

---

## 4. Implementation plan

### 4.1 Crate structure

```
Cargo.toml (workspace)
├── Cargo.lock
├── src/
│   ├── main.rs              # CLI entry point, clap arg parsing
│   ├── proxy.rs             # Core reverse proxy logic (tower::Service)
│   ├── recorder.rs          # File I/O: writes request/response JSONL files
│   ├── metadata.rs          # Client version detection, run classification
│   └── config.rs            # Configuration via CLI args + config file
├── tests/
│   └── integration.rs       # Integration test: proxy forwards correctly
├── examples/
│   └── capture.sh           # Example usage script
└── README.md                # Usage documentation
```

### 4.2 Dependencies (Cargo.toml)

| Crate | Purpose | Justification |
|---|---|---|
| `tokio` (feature: full) | Async runtime | Standard for axum/http services |
| `axum` + `tower` | HTTP server + middleware chain | Request/response manipulation, proxy forwarding |
| `hyper` | Low-level HTTP (via tower) | For transparent request forwarding with body access |
| `reqwest` (feature: streaming) | HTTP client for upstream | Forward captured requests to real Automate |
| `serde` + `serde_json` | JSON serialization of records | Structured metadata files |
| `clap` (derive) | CLI argument parsing | Standard Rust CLI framework |
| `uuid` (feature: v4) | Unique request IDs | Per-request file naming |
| `chrono` | Timestamp formatting | ISO 8601 timestamps in output |
| `tracing` + `tracing-subscriber` | Structured logging | Debugging and operational visibility |
| `thiserror` | Error types | Clean error handling |

**No filesystem watcher, no external API calls.** This is a standalone binary that writes to disk. All I/O is local.

### 4.3 Core proxy logic (proxy.rs)

```rust
// Pseudocode — not production code
async fn capture_handler(
    req: Request<Body>,
    upstream_url: Url,
    recorder: Arc<Recorder>,
) -> Result<Response<Body>> {
    // 1. Clone the request body so we can record it AND forward it
    let (parts, body) = req.into_parts();
    
    // 2. Read and buffer the full request body for recording
    let request_bytes = hyper::body::to_bytes(body).await?;
    
    // 3. Extract metadata BEFORE forwarding
    let meta = extract_metadata(&parts, &request_bytes);
    
    // 4. Record to /testdata/corpus/ asynchronously (non-blocking)
    recorder.record_request(&parts.method, &parts.uri.path(), 
                            &request_bytes, meta).await;
    
    // 5. Forward to upstream with cloned body
    let forward_req = Request::from_parts(parts, Body::from(request_bytes.clone()));
    let response = client.forward(forward_req).await?;
    
    // 6. Record the response
    let (resp_parts, resp_body) = response.into_parts();
    let resp_bytes = hyper::body::to_bytes(resp_body).await?;
    recorder.record_response(&resp_parts.status, &resp_bytes).await;
    
    // 7. Return cloned response to caller
    Ok(Response::from_parts(resp_parts, Body::from(resp_bytes)))
}
```

### 4.4 Recorder (recorder.rs)

- Creates a unique directory per captured message: `{output_dir}/{timestamp}-{uuid}/`
- Writes `request.jsonl` and `response.jsonl` atomically (write to temp file then rename)
- Maintains an in-memory count of total messages for metadata updates
- Runs on a separate tokio task to avoid blocking the proxy path

**Performance constraint:** Recording must add <1ms overhead per request. File writes are batched and non-blocking — the proxy path never waits for disk I/O.

### 4.5 Metadata extraction (metadata.rs)

```rust
pub struct CaptureMetadata {
    pub client_version: Option<String>,
    pub platform: PlatformInfo,
    pub run_type: RunType,
    pub node_name: String,
}

fn extract_metadata(req_parts: &http::request::Parts, body: &[u8]) -> CaptureMetadata {
    // 1. Parse JSON body (Chef data collector payloads are always JSON)
    let payload = serde_json::from_slice::<serde_json::Value>(body).ok();
    
    // 2. Extract client version from payload or headers
    let client_version = payload
        .as_ref()
        .and_then(|p| p["chef_implementation_version"].as_str())
        .or_else(|| /* check custom headers */)
        .map(String::from);
    
    // 3. Classify run type from path + body content
    let run_type = classify_run(req_parts.uri.path(), payload.as_ref());
    
    // 4. Extract platform info
    let platform = extract_platform(payload.as_ref());
    
    CaptureMetadata { client_version, platform, run_type, ... }
}
```

---

## 5. Verification plan (VERIFY loop)

### 5.1 Automated tests

| Test | Requirement | What it checks |
|---|---|---|
| `test_proxy_forwards_correctly` | Core proxy | Response from upstream is returned unchanged to client |
| `test_record_request_body` | Recording | Captured request body matches original exactly (byte-for-byte) |
| `test_record_response_body` | Recording | Captured response body matches original exactly |
| `test_metadata_extracted` | Metadata extraction | Client version, platform, and run type correctly classified from known payload |
| `test_corpus_structure` | File format | Output directory has correct structure: meta.json + request/response JSONL files |
| `test_concurrent_capture` | Performance | Proxy handles 50 concurrent requests without dropping any captures |
| `test_large_payloads` | ING-11 validation | Payloads up to configurable limit are captured; over-limit returns 413 |

### 5.2 Acceptance criteria (from spec)

- [ ] Captured corpus contains all required message types (success, failure, partial, compliance)
- [ ] Corpus covers ≥3 Chef client versions
- [ ] Corpus covers ≥4 platforms
- [ ] Spot-check payloads against Automate docs for structural validity
- [ ] Proxy adds <1ms latency to request path

---

## 6. Scale phase (SCALE loop)

After the core implementation passes verification:

1. **Tag corpus with version metadata** — add `corpus_version` field to meta.json that increments when new capture sessions start
2. **Document capture methodology** — write a `CAPTURE-METHOD.md` in `/testdata/corpus/` explaining how to reproduce each run type
3. **Add `--daemon` mode** — long-running daemon that can be started/stopped, writes continuous captures without restarting
4. **Corpus replay tool** — optional companion binary that replays captured corpus against a test ingest endpoint for validation

---

## 7. Blockers / Open Questions

| Question | Impact | Default assumption |
|---|---|---|
| Q1: Exact data collector token format? | Low | Token is passed as header `data-collector-token`; proxy forwards it unchanged |
| Q2: What path does Chef Infra Client use for compliance-only runs vs converge runs? | Medium — affects classification accuracy | `/checkins` = compliance, `/reports` = converge (to be validated against corpus) |
| Q3: Maximum expected payload size from a single node? | Low — informs 413 threshold | 32MB default (per ING-11); validate against captured payloads |

---

## 8. Guardrails

- **No sudo required** — proxy runs as a regular user with write access to the corpus directory
- **No network egress at runtime** — proxy only connects to its configured upstream; no telemetry, no external APIs
- **No secrets in code or logs** — tokens are redacted from metadata output
- **No rm -rf / (obvious)**

---

## 9. Task status ledger

| Milestone | Task | Status |
|---|---|---|
| M0 | M0-01 Corpus Capture Proxy | DESIGN COMPLETE → ready for EXECUTION on .14 (p40, 27b) |

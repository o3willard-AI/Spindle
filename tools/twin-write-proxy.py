#!/usr/bin/env python3
"""
Spindle Twin-Write Proxy
========================
Sits between Cinc Clients and the real Cinc Server. Forwards every data-collector
and InSpec payload to BOTH Spindle and the original Cinc Server simultaneously.
Spindle failures are logged but never block the primary Cinc Server flow.

Operators monitor GET /health until they're confident Spindle is processing
correctly, then cut over directly.

Port: 8081
"""

import asyncio
import hashlib
import json
import os
import time
from datetime import datetime, timezone

import httpx
from fastapi import FastAPI, Request, Response
from fastapi.responses import JSONResponse

# ── Config ──────────────────────────────────────────────────────────────────
SPINDLE_URL = os.getenv("SPINDLE_URL", "http://192.168.101.101:8080")
CINC_SERVER_URL = os.getenv("CINC_SERVER_URL", "https://192.168.101.220")
SPINDLE_TOKEN = os.getenv("SPINDLE_TOKEN", "spindle-dev-token")
VERIFY_TLS = os.getenv("VERIFY_TLS", "false").lower() == "true"

# ── State ───────────────────────────────────────────────────────────────────
app = FastAPI(title="Spindle Twin-Write Proxy")
client = httpx.AsyncClient(timeout=30.0, verify=VERIFY_TLS)

stats = {
    "started_at": datetime.now(timezone.utc).isoformat(),
    "spindle_success": 0,
    "spindle_failure": 0,
    "spindle_latency_ms": [],
    "cinc_success": 0,
    "cinc_failure": 0,
    "total_requests": 0,
    "last_spindle_error": None,
    "last_cinc_error": None,
    "recent_requests": [],  # last 20 for quick inspection
}

# ── Helpers ──────────────────────────────────────────────────────────────────

def make_receipt(body: bytes) -> str:
    """Generate a receipt token from the payload hash."""
    return hashlib.sha256(body).hexdigest()[:16]

# ── Routes ───────────────────────────────────────────────────────────────────

@app.get("/health")
async def health():
    """Operator dashboard — shows twin-write status at a glance."""
    recent = stats["recent_requests"][-5:]
    return {
        "status": "ok",
        "uptime_seconds": (
            datetime.now(timezone.utc)
            - datetime.fromisoformat(stats["started_at"])
        ).total_seconds(),
        "spindle": {
            "url": SPINDLE_URL,
            "success": stats["spindle_success"],
            "failure": stats["spindle_failure"],
            "success_rate": (
                f"{stats['spindle_success'] / max(stats['total_requests'], 1) * 100:.1f}%"
            ),
            "last_error": stats["last_spindle_error"],
        },
        "cinc_server": {
            "url": CINC_SERVER_URL,
            "success": stats["cinc_success"],
            "failure": stats["cinc_failure"],
            "last_error": stats["last_cinc_error"],
        },
        "total_requests": stats["total_requests"],
        "recent": [
            {
                "time": r["time"],
                "type": r["type"],
                "receipt": r["receipt"],
                "spindle": r["spindle"],
                "cinc": r["cinc"],
            }
            for r in recent
        ],
    }


@app.post("/ingest/events/data-collector")
async def data_collector(request: Request):
    """Forward Chef data-collector payloads to both systems."""
    return await _proxy(request, "data-collector")


@app.post("/ingest/events/inspec")
async def inspec(request: Request):
    """Forward InSpec reporter payloads to both systems."""
    return await _proxy(request, "inspec")


async def _proxy(request: Request, event_type: str):
    """Core twin-write logic."""
    body = await request.body()
    content_type = request.headers.get("content-type", "application/json")
    receipt = make_receipt(body)
    t0 = time.monotonic()

    result = {
        "time": datetime.now(timezone.utc).isoformat(),
        "type": event_type,
        "receipt": receipt,
        "size_bytes": len(body),
        "spindle": "pending",
        "cinc": "pending",
    }

    # Forward to Spindle and Cinc Server in parallel
    spindle_task = _forward_to_spindle(body, content_type, event_type)
    cinc_task = _forward_to_cinc(body, content_type)

    spindle_result, cinc_result = await asyncio.gather(spindle_task, cinc_task)

    result["spindle"] = spindle_result
    result["cinc"] = cinc_result
    result["latency_ms"] = round((time.monotonic() - t0) * 1000)

    # Track stats
    stats["total_requests"] += 1
    if spindle_result.startswith("202"):
        stats["spindle_success"] += 1
    else:
        stats["spindle_failure"] += 1
        stats["last_spindle_error"] = spindle_result

    if cinc_result.startswith("2"):
        stats["cinc_success"] += 1
    else:
        stats["cinc_failure"] += 1
        stats["last_cinc_error"] = cinc_result

    # Keep last 20 for inspection
    stats["recent_requests"].append(result)
    if len(stats["recent_requests"]) > 20:
        stats["recent_requests"] = stats["recent_requests"][-20:]

    # Return the Spindle response to the client (transparent proxy)
    if spindle_result.startswith("202"):
        return Response(
            content=json.dumps({"receipt": receipt, "status": "accepted"}),
            status_code=202,
            media_type="application/json",
        )
    else:
        return JSONResponse(
            content={"error": "spindle_unavailable", "receipt": receipt},
            status_code=502,
        )


async def _forward_to_spindle(body: bytes, content_type: str, event_type: str) -> str:
    """Forward payload to Spindle ingest."""
    headers = {
        "Authorization": f"Bearer {SPINDLE_TOKEN}",
        "Content-Type": content_type,
        "X-Spindle-Event-Type": event_type,
    }
    try:
        url = f"{SPINDLE_URL}/ingest/events/{event_type.replace('-', '_')}"
        # Map: data-collector → data_collector
        url = url.replace("data-collector", "data-collector").replace("_", "-")
        # Actually use the correct path
        path = "data-collector" if event_type == "data-collector" else "inspec"
        r = await client.post(
            f"{SPINDLE_URL}/ingest/events/{path}",
            content=body,
            headers=headers,
        )
        return f"{r.status_code} ({len(body)} bytes)"
    except Exception as e:
        return f"error: {e}"


async def _forward_to_cinc(body: bytes, content_type: str) -> str:
    """Forward payload to the original Cinc Server."""
    headers = {
        "Content-Type": content_type,
    }
    try:
        # Cinc Server data-collector endpoint
        url = f"{CINC_SERVER_URL}/data-collector/v0"
        r = await client.post(url, content=body, headers=headers)
        return f"{r.status_code} ({len(body)} bytes)"
    except Exception as e:
        return f"error: {e}"


# ── Entrypoint ───────────────────────────────────────────────────────────────
if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=8081)

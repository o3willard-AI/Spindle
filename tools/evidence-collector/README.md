# Spindle Evidence Collector

Report Builder — turns Spindle's API data into user-facing evidence views.

## What it does

Polls Spindle's API at `192.0.2.10:8080` every 30 seconds and tracks state changes over time. When Deployment Engineer's chaos cycle runs, it captures: node drifts out of compliance → Cinc Auditor detects → Cinc converges → compliance restored.

## Three views

1. **Timeline** (`timeline.html` / `timeline.json`) — chronological event log showing node seen events, run events, compliance reports, and control results in temporal order.

2. **Fleet Dashboard** (`fleet.html` / `fleet.json`) — per-node health summary: platform, policy group, status (healthy/degraded/converging), health score, run counts, resource update/failed/skipped counts, last converge time, compliance status.

3. **Detail View** (`detail_{node_id}.html` / `detail_{node_id}.json`) — single node deep-dive: full node attributes, all runs with resource event details, compliance history, cookbook usage.

A combined `report.html` is also generated with all views + CSS styling.

## Usage

```bash
# Install dependencies
pip install -r requirements.txt  # or: pip install requests jinja2

# Run once
python run.py --once

# Run continuously (polls every 30s)
python run.py

# Custom API base / token / interval
python run.py --api-base http://192.0.2.10:8080 --token spindle-dev-token --interval 15

# Detail view for a specific node
python run.py --once --node-id 868a6e39-e5cc-485e-a8b0-6763bec84687
```

## Output

All output files are written to `./output/` by default. Use `--output-dir` to change.

## Requirements

- Python 3.10+
- `requests` (HTTP client)
- `jinja2` (templating, optional — falls back to string formatting)

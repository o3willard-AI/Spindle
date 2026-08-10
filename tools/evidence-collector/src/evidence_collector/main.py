"""Spindle Evidence Collector — Report Builder.

Polls Spindle's API at 198.51.100.101:8080 and generates three views:
1. Timeline — chronological event log
2. Fleet dashboard — per-node health
3. Detail view — single node deep-dive

Usage:
    python -m evidence_collector.main [--once] [--interval 30] [--output-dir ./output]
"""

import argparse
import json
import logging
import os
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import requests

logger = logging.getLogger(__name__)

DEFAULT_API_BASE = "http://198.51.100.101:8080"
DEFAULT_TOKEN = "spindle-dev-token"
DEFAULT_OUTPUT_DIR = "./output"
DEFAULT_INTERVAL = 30


class SpindleAPI:
    """Client for the Spindle API."""

    def __init__(self, base_url: str, token: str, timeout: int = 10):
        self.base_url = base_url.rstrip("/")
        self.headers = {
            "Authorization": f"Bearer {token}",
            "Accept": "application/json",
        }
        self.timeout = timeout

    def _get(self, path: str, params: dict | None = None) -> dict[str, Any]:
        url = f"{self.base_url}{path}"
        resp = requests.get(url, headers=self.headers, params=params, timeout=self.timeout)
        resp.raise_for_status()
        return resp.json()

    def get_nodes(self) -> list[dict]:
        """Fetch all nodes."""
        data = self._get("/v1/nodes", params={"limit": 1000})
        return data.get("data", [])

    def get_node(self, node_id: str) -> dict:
        """Fetch a single node by ID."""
        data = self._get(f"/v1/nodes/{node_id}")
        return data.get("data", {})

    def get_runs(self, node_id: str | None = None) -> list[dict]:
        """Fetch runs, optionally filtered by node."""
        params = {"limit": 1000}
        if node_id:
            params["node_id"] = node_id
        data = self._get("/v1/runs", params=params)
        return data.get("data", [])

    def get_run(self, run_id: str) -> dict:
        """Fetch a single run by ID with resource events."""
        data = self._get(f"/v1/runs/{run_id}")
        return data.get("data", {})

    def get_compliance_reports(self) -> list[dict]:
        """Fetch compliance reports."""
        data = self._get("/v1/compliance/reports", params={"limit": 1000})
        return data.get("data", {}).get("items", [])

    def get_compliance_controls(self) -> list[dict]:
        """Fetch compliance control results."""
        data = self._get("/v1/compliance/controls", params={"limit": 1000})
        return data.get("data", {}).get("items", [])

    def get_resource_event_aggregates(self) -> list[dict]:
        """Fetch resource event aggregates."""
        data = self._get("/v1/resource-events/aggregates", params={"limit": 1000})
        return data.get("data", [])

    def get_health(self) -> dict:
        """Fetch system health."""
        return self._get("/v1/health")

    def get_cookbook_usage(self) -> list[dict]:
        """Fetch cookbook usage data."""
        data = self._get("/v1/cookbooks", params={"limit": 1000})
        return data.get("data", [])

    def get_node_compliance_status(self, node_id: str) -> dict:
        """Fetch compliance status for a specific node."""
        try:
            data = self._get(f"/v1/compliance/nodes/{node_id}/status")
            return data.get("data", {})
        except requests.HTTPError:
            return {}

    def get_node_runs(self, node_id: str) -> list[dict]:
        """Fetch runs for a specific node."""
        data = self._get(f"/v1/nodes/{node_id}/runs", params={"limit": 1000})
        return data.get("data", [])

    def get_node_resource_events(self, node_id: str) -> list[dict]:
        """Fetch resource events for a specific node."""
        data = self._get(f"/v1/nodes/{node_id}/resource-events", params={"limit": 1000})
        return data.get("data", [])

    def get_node_compliance_history(self, node_id: str) -> list[dict]:
        """Fetch compliance history for a specific node."""
        try:
            data = self._get(f"/v1/compliance/nodes/{node_id}/history", params={"limit": 1000})
            return data.get("data", [])
        except requests.HTTPError:
            return []


class EvidenceStore:
    """Tracks state changes over time by polling the API periodically."""

    def __init__(self, api: SpindleAPI, output_dir: Path):
        self.api = api
        self.output_dir = output_dir
        self.output_dir.mkdir(parents=True, exist_ok=True)
        self.history: list[dict] = []

    def poll(self) -> dict:
        """Poll all endpoints and return a snapshot."""
        snapshot = {
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "nodes": self.api.get_nodes(),
            "runs": self.api.get_runs(),
            "compliance_reports": self.api.get_compliance_reports(),
            "compliance_controls": self.api.get_compliance_controls(),
            "resource_event_aggregates": self.api.get_resource_event_aggregates(),
            "health": self.api.get_health(),
        }
        self.history.append(snapshot)
        # Keep last 100 snapshots
        if len(self.history) > 100:
            self.history = self.history[-100:]
        return snapshot

    def save_snapshot(self, snapshot: dict) -> None:
        """Save a snapshot to disk as JSON."""
        ts = snapshot["timestamp"].replace(":", "-").replace(".", "_")
        path = self.output_dir / f"snapshot_{ts}.json"
        with open(path, "w") as f:
            json.dump(snapshot, f, indent=2, default=str)

    def get_all_nodes(self) -> list[dict]:
        """Get all nodes from the latest snapshot."""
        if not self.history:
            return []
        return self.history[-1].get("nodes", [])

    def get_all_runs(self) -> list[dict]:
        """Get all runs from the latest snapshot."""
        if not self.history:
            return []
        return self.history[-1].get("runs", [])

    def get_compliance_reports(self) -> list[dict]:
        """Get compliance reports from the latest snapshot."""
        if not self.history:
            return []
        return self.history[-1].get("compliance_reports", [])

    def get_compliance_controls(self) -> list[dict]:
        """Get compliance control results from the latest snapshot."""
        if not self.history:
            return []
        return self.history[-1].get("compliance_controls", [])

    def get_resource_event_aggregates(self) -> list[dict]:
        """Get resource event aggregates from the latest snapshot."""
        if not self.history:
            return []
        return self.history[-1].get("resource_event_aggregates", [])

    def get_node_history(self, node_id: str) -> list[dict]:
        """Get all snapshots that contain this node."""
        return [
            s for s in self.history
            if any(n.get("id") == node_id for n in s.get("nodes", []))
        ]

    def get_node_run_history(self, node_id: str) -> list[dict]:
        """Get all runs for a node across snapshots."""
        runs = []
        for s in self.history:
            for r in s.get("runs", []):
                if r.get("node_id") == node_id:
                    runs.append(r)
        return runs

    def get_node_compliance_history(self, node_id: str) -> list[dict]:
        """Get compliance reports for a node across snapshots."""
        reports = []
        for s in self.history:
            for r in s.get("compliance_reports", []):
                if r.get("node_id") == node_id:
                    reports.append(r)
        return reports


class TimelineView:
    """View 1: Chronological event log."""

    def __init__(self, store: EvidenceStore):
        self.store = store

    def build(self) -> list[dict]:
        """Build a chronological timeline of events from all data sources."""
        events: list[dict] = []

        # Node events (last_seen changes)
        for node in self.store.get_all_nodes():
            events.append({
                "timestamp": node.get("last_seen", ""),
                "type": "node_seen",
                "node_id": node.get("id", ""),
                "node_name": node.get("name", ""),
                "description": f"Node {node.get('name', 'unknown')} last seen",
                "severity": "info",
            })

        # Run events
        for run in self.store.get_all_runs():
            status = run.get("status", "unknown")
            severity = "info" if status == "success" else "warning" if status == "failure" else "error"
            events.append({
                "timestamp": run.get("start_time", ""),
                "type": "run",
                "node_id": run.get("node_id", ""),
                "run_id": run.get("run_id", ""),
                "description": f"Run {run.get('run_id', '')} on node {run.get('node_id', '')}: {status} "
                              f"(updated={run.get('updated_count', 0)}, failed={run.get('failed_count', 0)})",
                "severity": severity,
            })

        # Compliance report events
        for report in self.store.get_compliance_reports():
            events.append({
                "timestamp": report.get("created_at", ""),
                "type": "compliance",
                "node_id": report.get("node_id", ""),
                "description": f"Compliance: {report.get('status', 'unknown')} "
                            f"(passed={report.get('passed_count', 0)}, failed={report.get('failed_count', 0)})",
                "severity": "info" if report.get("status") == "passed" else "warning",
            })

        # Control result events
        for control in self.store.get_compliance_controls():
            events.append({
                "timestamp": control.get("created_at", ""),
                "type": "control",
                "node_id": control.get("node_id", ""),
                "description": f"Control {control.get('control_id', '')}: {control.get('status', 'unknown')}",
                "severity": "info" if control.get("status") == "passed" else "warning",
            })

        # Sort by timestamp
        events.sort(key=lambda e: e.get("timestamp", ""))

        return events

    def render_html(self) -> str:
        """Render the timeline as HTML."""
        events = self.build()
        if not events:
            return "<p>No events recorded yet. Waiting for data from chaos cycle...</p>"

        rows = []
        for ev in events:
            ts = ev.get("timestamp", "")
            # Format timestamp
            try:
                dt = datetime.fromisoformat(ts.replace("Z", "+00:00"))
                ts_formatted = dt.strftime("%H:%M:%S")
            except (ValueError, AttributeError):
                ts_formatted = ts

            severity_class = {
                "info": "timeline-info",
                "warning": "timeline-warning",
                "error": "timeline-error",
            }.get(ev.get("severity", "info"), "timeline-info")

            rows.append(f"""
                <tr class="{severity_class}">
                    <td>{ts_formatted}</td>
                    <td><span class="badge bg-{severity_class}">{ev['type']}</span></td>
                    <td>{ev.get('node_name', ev.get('node_id', ''))}</td>
                    <td>{ev['description']}</td>
                </tr>""")

        return f"""
        <div class="view timeline-view">
            <h2>Timeline — Chronological Event Log</h2>
            <table class="table table-sm table-hover">
                <thead>
                    <tr>
                        <th>Time</th>
                        <th>Type</th>
                        <th>Node</th>
                        <th>Description</th>
                    </tr>
                </thead>
                <tbody>
                    {''.join(rows)}
                </tbody>
            </table>
        </div>"""

    def render_json(self) -> str:
        """Render the timeline as JSON."""
        return json.dumps(self.build(), indent=2, default=str)


class FleetDashboardView:
    """View 2: Per-node health dashboard."""

    def __init__(self, store: EvidenceStore, api: SpindleAPI):
        self.store = store
        self.api = api

    def build(self) -> list[dict]:
        """Build fleet health data for all nodes."""
        nodes = self.store.get_all_nodes()
        runs = self.store.get_all_runs()
        aggregates = self.store.get_resource_event_aggregates()

        # Group runs by node
        runs_by_node: dict[str, list[dict]] = {}
        for run in runs:
            nid = run.get("node_id", "")
            runs_by_node.setdefault(nid, []).append(run)

        # Build per-node health
        fleet: list[dict] = []
        for node in nodes:
            nid = node.get("id", "")
            node_runs = runs_by_node.get(nid, [])

            # Compute health score
            total = sum(r.get("total_resource_count", 0) for r in node_runs)
            failed = sum(r.get("failed_count", 0) for r in node_runs)
            updated = sum(r.get("updated_count", 0) for r in node_runs)
            skipped = sum(r.get("skipped_count", 0) for r in node_runs)
            up_to_date = total - updated - failed - skipped

            if total > 0:
                health_score = round((up_to_date / total) * 100, 1)
            else:
                health_score = 100.0

            last_run = node_runs[-1] if node_runs else None
            last_converge = last_run.get("start_time", "") if last_run else None

            # Determine status
            if last_run:
                if last_run.get("status") == "success" and last_run.get("failed_count", 0) == 0:
                    status = "healthy"
                elif last_run.get("failed_count", 0) > 0:
                    status = "degraded"
                else:
                    status = "converging"
            else:
                status = "unknown"

            # Get compliance info
            compliance = self.api.get_node_compliance_status(nid)

            fleet.append({
                "node_id": nid,
                "node_name": node.get("name", "unknown"),
                "node_type": node.get("node_type", ""),
                "platform": node.get("platform", ""),
                "policy_group": node.get("policy_group", ""),
                "status": status,
                "health_score": health_score,
                "last_converge": last_converge,
                "total_resources": total,
                "updated": updated,
                "failed": failed,
                "skipped": skipped,
                "up_to_date": up_to_date,
                "run_count": len(node_runs),
                "compliance_status": compliance.get("status", "unknown"),
            })

        return fleet

    def render_html(self) -> str:
        """Render the fleet dashboard as HTML."""
        fleet = self.build()
        if not fleet:
            return "<p>No nodes found. Waiting for data from chaos cycle...</p>"

        rows = []
        for node in fleet:
            status_class = {
                "healthy": "status-healthy",
                "degraded": "status-degraded",
                "converging": "status-converging",
                "unknown": "status-unknown",
            }.get(node["status"], "status-unknown")

            rows.append(f"""
                <tr class="{status_class}">
                    <td><strong>{node['node_name']}</strong></td>
                    <td>{node['platform']}</td>
                    <td>{node['policy_group']}</td>
                    <td><span class="badge bg-{status_class}">{node['status']}</span></td>
                    <td>{node['health_score']}%</td>
                    <td>{node['run_count']}</td>
                    <td>{node['updated']}</td>
                    <td class="text-danger">{node['failed']}</td>
                    <td>{node['up_to_date']}</td>
                    <td>{node['last_converge'] or 'never'}</td>
                    <td>{node['compliance_status']}</td>
                </tr>""")

        return f"""
        <div class="view fleet-dashboard">
            <h2>Fleet Dashboard — Per-Node Health</h2>
            <table class="table table-sm table-hover">
                <thead>
                    <tr>
                        <th>Node</th>
                        <th>Platform</th>
                        <th>Policy Group</th>
                        <th>Status</th>
                        <th>Health Score</th>
                        <th>Runs</th>
                        <th>Updated</th>
                        <th>Failed</th>
                        <th>Up-to-Date</th>
                        <th>Last Converge</th>
                        <th>Compliance</th>
                    </tr>
                </thead>
                <tbody>
                    {''.join(rows)}
                </tbody>
            </table>
        </div>"""

    def render_json(self) -> str:
        """Render the fleet dashboard as JSON."""
        return json.dumps(self.build(), indent=2, default=str)


class DetailView:
    """View 3: Single node deep-dive."""

    def __init__(self, store: EvidenceStore, api: SpindleAPI):
        self.store = store
        self.api = api

    def build(self, node_id: str) -> dict:
        """Build detailed view for a single node."""
        node = self.api.get_node(node_id)
        runs = self.api.get_runs(node_id=node_id)

        # Enrich runs with resource event details
        detailed_runs = []
        for run in runs:
            run_detail = self.api.get_run(run.get("id", ""))
            detailed_runs.append(run_detail)

        # Get compliance history
        compliance_history = self.store.get_node_compliance_history(node_id)

        # Get cookbook usage
        cookbook_usage = self.api.get_cookbook_usage()

        return {
            "node": node,
            "runs": detailed_runs,
            "compliance_history": compliance_history,
            "cookbook_usage": cookbook_usage,
        }

    def render_html(self, node_id: str) -> str:
        """Render the detail view as HTML."""
        try:
            data = self.build(node_id)
        except requests.HTTPError as e:
            return f"<p>Error fetching node {node_id}: {e}</p>"

        node = data["node"]
        runs = data["runs"]

        # Node header
        header = f"""
        <div class="view detail-view">
            <h2>Detail View — {node.get('name', 'Unknown Node')}</h2>
            <div class="card mb-3">
                <div class="card-body">
                    <h5 class="card-title">{node.get('name', '')}</h5>
                    <p class="card-text">
                        <strong>Platform:</strong> {node.get('platform', '')} |
                        <strong>Policy Group:</strong> {node.get('policy_group', '')} |
                        <strong>Status:</strong> {node.get('status', '')} |
                        <strong>Last Seen:</strong> {node.get('last_seen', '')}
                    </p>
                </div>
            </div>"""

        # Runs section
        if runs:
            run_rows = []
            for run in runs:
                events = run.get("resource_events", {}).get("items", [])
                event_summary = ", ".join(
                    f"{e.get('resource_name', '')}: {e.get('status', '')}"
                    for e in events[:5]
                )
                if len(events) > 5:
                    event_summary += f" ... +{len(events) - 5} more"

                run_rows.append(f"""
                    <tr>
                        <td>{run.get('run_id', '')}</td>
                        <td>{run.get('status', '')}</td>
                        <td>{run.get('start_time', '')}</td>
                        <td>{run.get('duration_ms', 0)} ms</td>
                        <td>{run.get('total_resource_count', 0)}</td>
                        <td>{run.get('updated_count', 0)}</td>
                        <td class="text-danger">{run.get('failed_count', 0)}</td>
                        <td>{event_summary}</td>
                    </tr>""")

            runs_section = f"""
            <h3>Runs ({len(runs)})</h3>
            <table class="table table-sm table-bordered">
                <thead>
                    <tr>
                        <th>Run ID</th>
                        <th>Status</th>
                        <th>Start Time</th>
                        <th>Duration</th>
                        <th>Total Resources</th>
                        <th>Updated</th>
                        <th>Failed</th>
                        <th>Resource Events</th>
                    </tr>
                </thead>
                <tbody>
                    {''.join(run_rows)}
                </tbody>
            </table>"""
        else:
            runs_section = "<h3>Runs</h3><p>No runs found for this node.</p>"

        # Compliance history
        if data["compliance_history"]:
            comp_rows = []
            for comp in data["compliance_history"]:
                comp_rows.append(f"""
                    <tr>
                        <td>{comp.get('created_at', '')}</td>
                        <td>{comp.get('status', '')}</td>
                        <td>{comp.get('passed_count', 0)}</td>
                        <td class="text-danger">{comp.get('failed_count', 0)}</td>
                    </tr>""")
            comp_section = f"""
            <h3>Compliance History ({len(data['compliance_history'])})</h3>
            <table class="table table-sm table-bordered">
                <thead>
                    <tr>
                        <th>Timestamp</th>
                        <th>Status</th>
                        <th>Passed</th>
                        <th>Failed</th>
                    </tr>
                </thead>
                <tbody>
                    {''.join(comp_rows)}
                </tbody>
            </table>"""
        else:
            comp_section = "<h3>Compliance History</h3><p>No compliance reports found.</p>"

        # Cookbook usage
        cb = data.get("cookbook_usage", [])
        if cb:
            cb_rows = []
            for c in cb:
                cb_rows.append(f"""
                    <tr>
                        <td>{c.get('name', '')}</td>
                        <td>{c.get('version', '')}</td>
                        <td>{c.get('node_count', 0)}</td>
                        <td>{c.get('last_seen', '')}</td>
                    </tr>""")
            cb_section = f"""
            <h3>Cookbook Usage ({len(cb)})</h3>
            <table class="table table-sm table-bordered">
                <thead>
                    <tr>
                        <th>Cookbook</th>
                        <th>Version</th>
                        <th>Nodes</th>
                        <th>Last Seen</th>
                    </tr>
                </thead>
                <tbody>
                    {''.join(cb_rows)}
                </tbody>
            </table>"""
        else:
            cb_section = "<h3>Cookbook Usage</h3><p>No cookbook data available.</p>"

        return f"""{header}
            {runs_section}
            {comp_section}
            {cb_section}
        </div>"""

    def render_json(self, node_id: str) -> str:
        """Render the detail view as JSON."""
        try:
            data = self.build(node_id)
        except requests.HTTPError as e:
            return json.dumps({"error": str(e)})
        return json.dumps(data, indent=2, default=str)


class ReportBuilder:
    """Orchestrates polling and rendering of all views."""

    def __init__(self, api: SpindleAPI, output_dir: Path):
        self.api = api
        self.output_dir = output_dir
        self.store = EvidenceStore(api, output_dir)
        self.timeline = TimelineView(self.store)
        self.fleet = FleetDashboardView(self.store, api)
        self.detail = DetailView(self.store, api)

    def poll_once(self) -> dict:
        """Poll the API once and save the snapshot."""
        snapshot = self.store.poll()
        self.store.save_snapshot(snapshot)
        return snapshot

    def render_all(self, node_id: str | None = None) -> None:
        """Render all views to HTML and JSON."""
        self.output_dir.mkdir(parents=True, exist_ok=True)

        # Timeline
        timeline_html = self.timeline.render_html()
        timeline_json = self.timeline.render_json()
        (self.output_dir / "timeline.html").write_text(timeline_html)
        (self.output_dir / "timeline.json").write_text(timeline_json)

        # Fleet dashboard
        fleet_html = self.fleet.render_html()
        fleet_json = self.fleet.render_json()
        (self.output_dir / "fleet.html").write_text(fleet_html)
        (self.output_dir / "fleet.json").write_text(fleet_json)

        # Detail view
        if node_id:
            detail_html = self.detail.render_html(node_id)
            detail_json = self.detail.render_json(node_id)
            (self.output_dir / f"detail_{node_id}.html").write_text(detail_html)
            (self.output_dir / f"detail_{node_id}.json").write_text(detail_json)
        else:
            # Render detail for all nodes
            for node in self.store.get_all_nodes():
                nid = node.get("id", "")
                if nid:
                    detail_html = self.detail.render_html(nid)
                    detail_json = self.detail.render_json(nid)
                    (self.output_dir / f"detail_{nid}.html").write_text(detail_html)
                    (self.output_dir / f"detail_{nid}.json").write_text(detail_json)

        # Combined report
        self._render_combined_report()

    def _render_combined_report(self) -> None:
        """Render a combined HTML report with all views."""
        css = """
        <style>
            body { font-family: 'Segoe UI', Tahoma, sans-serif; margin: 20px; background: #f5f5f5; }
            h1 { color: #333; border-bottom: 3px solid #007bff; padding-bottom: 10px; }
            h2 { color: #007bff; margin-top: 30px; }
            h3 { color: #666; }
            .view { background: white; padding: 20px; margin: 15px 0; border-radius: 8px; box-shadow: 0 2px 4px rgba(0,0,0,0.1); }
            table { width: 100%; border-collapse: collapse; }
            th { background: #e9ecef; padding: 8px; text-align: left; border: 1px solid #dee2e6; }
            td { padding: 6px 8px; border: 1px solid #dee2e6; }
            .table-sm td { padding: 4px 6px; font-size: 0.875em; }
            .timeline-info { background: #d1ecf1; }
            .timeline-warning { background: #fff3cd; }
            .timeline-error { background: #f8d7da; }
            .status-healthy { color: #28a745; }
            .status-degraded { color: #ffc107; }
            .status-converging { color: #17a2b8; }
            .status-unknown { color: #6c757d; }
            .badge { padding: 2px 8px; border-radius: 4px; font-size: 0.75em; }
            .text-danger { color: #dc3545; }
            .card { border: 1px solid #dee2e6; border-radius: 8px; }
            .card-body { padding: 15px; }
            .card-title { font-size: 1.2em; margin-bottom: 10px; }
            .card-text { color: #6c757d; }
            .refresh-info { color: #6c757d; font-size: 0.85em; }
        </style>
        """

        timeline_html = self.timeline.render_html()
        fleet_html = self.fleet.render_html()

        # Get all node detail views
        detail_views = []
        for node in self.store.get_all_nodes():
            nid = node.get("id", "")
            if nid:
                try:
                    detail_views.append(self.detail.render_html(nid))
                except Exception:
                    pass

        html = f"""<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Spindle Evidence Report</title>
    {css}
</head>
<body>
    <h1>Spindle Evidence Report</h1>
    <p class="refresh-info">Generated: {datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")}</p>
    {timeline_html}
    {fleet_html}
    {''.join(detail_views)}
</body>
</html>"""

        (self.output_dir / "report.html").write_text(html)

    def run_continuous(self, interval: int = DEFAULT_INTERVAL) -> None:
        """Run continuously, polling and rendering at the given interval."""
        logger.info("Starting continuous evidence collection (interval=%ds)", interval)

        while True:
            try:
                self.poll_once()
                self.render_all()
                logger.info("Evidence report rendered to %s", self.output_dir)
            except Exception as e:
                logger.error("Error during poll/render cycle: %s", e, exc_info=True)

            time.sleep(interval)


def main():
    parser = argparse.ArgumentParser(description="Spindle Evidence Collector")
    parser.add_argument("--api-base", default=DEFAULT_API_BASE, help="API base URL")
    parser.add_argument("--token", default=DEFAULT_TOKEN, help="Bearer token")
    parser.add_argument("--output-dir", default=DEFAULT_OUTPUT_DIR, help="Output directory")
    parser.add_argument("--interval", type=int, default=DEFAULT_INTERVAL, help="Poll interval (seconds)")
    parser.add_argument("--once", action="store_true", help="Run once and exit")
    parser.add_argument("--node-id", help="Render detail view for specific node only")

    args = parser.parse_args()

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(message)s",
    )

    api = SpindleAPI(args.api_base, args.token)
    output_dir = Path(args.output_dir)

    builder = ReportBuilder(api, output_dir)

    if args.once:
        builder.poll_once()
        builder.render_all(node_id=args.node_id)
        logger.info("Single render complete. Output in %s", output_dir)
    else:
        builder.run_continuous(interval=args.interval)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""inspec_json_status.py — parse cinc-auditor InSpec JSON reporter output.

The cinc-auditor (InSpec) JSON reporter schema nests controls under
profiles[].controls[], and each control has a results[] array whose entries
carry status in ('passed','failed','skipped'). "errored" results are reported
with status 'failed' plus an exception, or the control itself fails to load.

Counts a control as "failed" if ANY of its results is status=failed (or the
control failed to load). Skips don't count. Prints three space-separated ints:
  <failed_controls> <total_controls> <skipped_controls>
"""
import json, sys

def load(path):
    with open(path) as f:
        return json.load(f)

def main(path):
    d = load(path)
    failed = total = skipped = 0
    # Controls may sit at top-level (old schema) or under profiles[] (current).
    profiles = d.get("profiles", []) or []
    for prof in profiles:
        for c in prof.get("controls", []) or []:
            total += 1
            results = c.get("results", [])
            if not results:
                # Control with no results but a non-passing profile status
                if c.get("status") in ("failed", "error"):
                    failed += 1
                continue
            if any(r.get("status") in ("failed", "error") for r in results):
                failed += 1
            if all(r.get("status") == "skipped" for r in results):
                skipped += 1
            elif any(r.get("status") == "skipped" for r in results):
                # partially skipped, has some real outcome; don't force-fail
                pass
    # Also account for top-level controls (fallback compat)
    for c in d.get("controls", []) or []:
        total += 1
        results = c.get("results", [])
        if any(r.get("status") in ("failed", "error") for r in results):
            failed += 1
        if all(r.get("status") == "skipped" for r in results):
            skipped += 1
    print(f"{failed} {total} {skipped}")

if __name__ == "__main__":
    main(sys.argv[1])
# Mem0 — Shared Project Memory

Semantic memory service at `http://192.168.101.42:8787`.

## API

### Store a fact
```python
import json, urllib.request

req_data = json.dumps({"fact": "your fact here", "agent_id": "sergey"}).encode()
req = urllib.request.Request(
    "http://192.168.101.42:8787/mem0/add",
    data=req_data, headers={"Content-Type": "application/json"}
)
resp = urllib.request.urlopen(req)
print(resp.read().decode())  # {"status":"stored","fact":"...","agent_id":"sergey"}
```

### Search
```python
req_data = json.dumps({"query": "spindle workspace", "limit": 5}).encode()
req = urllib.request.Request(
    "http://192.168.101.42:8787/mem0/search",
    data=req_data, headers={"Content-Type": "application/json"}
)
resp = urllib.request.urlopen(req)
results = json.loads(resp.read())  # {"results": [{"id": "...", "memory": "..."}, ...]}
```

## When to use

- **Before starting a task:** `search(query="<task topic>")` to recall past decisions
- **After completing a task:** `add(fact="<key learnings>")` so future sessions don't re-discover the same problems

## What to store

- Architecture decisions and rationale
- Gotchas and workarounds
- Cross-component contracts
- Config values and conventions
- Lessons learned from failures

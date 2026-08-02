# Sergey's Spindle Execution Architecture
## Agent Loop Best-Practices Conformance

### Model Pipeline (Hot-Path Optimized)

```
PLANNING PHASE                      EXECUTION PHASE
Sergey (.53/35b, reasoning ON)      Sergey (.14/27b, reasoning OFF)
    │                                    │
    │  Deep analysis, design, specs      │  Write code, run tests, debug
    │  ↓                                 │  ↓
    │  DESIGN.md (per component)         │  PR opened + tests green
    │                                    │
    └──────── handoff ──────────────────►│
                                         │
REVIEW PHASE                            │
Sergey (.53/35b, reasoning ON) ◄────────┘
    │
    │  Self-review vs spec + tests
    │  ↓
    │  APPROVED → Hephaestus sign-off (C8/C9/C10)
    │  REJECTED → back to execution with fix notes
    │
    └─── Hephaestus sign-off → MERGE
```

### Four Loops Per Task (Steinberger Pattern)

```
BUILD                     VERIFY                     FIX                       SCALE
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│ Sergey (27b)     │     │ Sergey (35b)    │     │ Sergey (27b)    │     │ Sergey (27b)    │
│ writes code +    │────▶│ runs full test  │────▶│ addresses all   │────▶│ cross-component │
│ tests from the   │     │ suite + spec-   │     │ review findings │     │ integration     │
│ DESIGN.md plan   │     │ compliance      │     │ until green     │     │ tests + rustdoc │
│                  │     │ audit           │     │                  │     │                  │
│ Deliverable: PR  │     │ Deliverable:    │     │ Deliverable:    │     │ Deliverable:    │
│ with unit tests  │     │ review report   │     │ updated PR      │     │ merged component│
└─────────────────┘     └─────────────────┘     └─────────────────┘     └─────────────────┘
```

**Critical rule:** BUILD happens with 27b (reasoning OFF) — no analysis paralysis, just code. VERIFY happens with 35b (reasoning ON) — this is where deep reasoning about correctness matters.

### Context Management Strategy

**Rule: ONE FRESH SESSION PER TASK.** Each task starts with a clean context containing only:
1. The Spindle engineering spec (as immutable reference)
2. The DESIGN.md for the specific component being built
3. The Plans.md task ledger (what's done, in progress, blocked)
4. The tool definitions Sergey needs

No accumulated history from previous tasks. No stale context. This is the single most important architectural decision — it prevents context drift across 80+ tasks and keeps each session's context fill ratio below the 80% compression threshold for the entire task.

### Guardrails (Pillar 3)

Sergey's explicit deny-rules while building Spindle:

```yaml
guardrails:
  - deny: "git push --force*"
    reason: "never rewrite shared history"
  - deny: "rm -rf /*"
    reason: "system destruction"
  - deny: "sudo *"
    reason: "no privilege escalation needed"
  - deny: "*.env|*secret*|*credential*|*token*"
    reason: "never commit secrets"
  - deny: "git push origin main --force"
    reason: "protected branch"
  - deny: "chmod 777 *"
    reason: "permissive file permissions"
  - deny: "curl/wget to external hosts"
    reason: "air-gap constraint — no runtime internet"
```

### Stopping Conditions (Pillar 4)

Per task, per loop iteration:

| Condition | Action |
|---|---|
| All tests green + review passed | → Task COMPLETE, advance Plans.md |
| 3 consecutive test failures | → Switch to 35b for diagnosis, then fix loop |
| > 15 execution iterations (27b) | → Escalate to Hephaestus for unblocking |
| > 1,000,000 tokens in session | → Force context reset, report to Hephaestus |
| Natural completion (agent stops calling tools) | → Verify state, mark complete or escalate |
| Guardrail violation detected | → Halt task, log violation, notify Hephaestus |

### Task Ledger (Contract-First)

`Plans.md` — maintained in repo root, updated after every task:

```markdown
## In Progress
- [ ] C2-01: Raw archive writer (Sergey)

## Done (84/84)
- [x] M0-01: Repo skeleton + CI
- [x] M0-02: Docker Compose test infra (Postgres + MinIO)
- [x] M0-03: Corpus capture (ING-03)

## Blocked
- (nothing)

## Needs Hephaestus Review
- (nothing)
```

### Self-Improvement Loop

After every 3 completed tasks, Sergey (35b) runs a brief retrospective:

```
analyze → pattern_detect → improve_task_template → update_DESIGN_standards
```

This feeds back into future DESIGN.md quality, catching anti-patterns before they propagate across components.

### Observability

Per task, Sergey logs:

```yaml
task_id: "C2-01"
iterations: 8
tokens_used: 342_000
tokens_planning: 89_000
tokens_execution: 253_000
wall_time_minutes: 47
tool_calls: 53
errors: 2
consecutive_failures_max: 2
compression_events: 0
result: "merged"
```

Rolled up per milestone for Hephaestus review.

### Build Sequence

Adhering to spec §6, adapted for single-agent Rust execution:

| Milestone | Duration | Tasks | Gates | Model |
|---|---|---|---|---|
| M0: Foundation | 1-2 days | 8-10 | Green CI + corpus captured | 27b |
| M1: Ingest to Storage | 3-4 days | 18-22 | Corpus replays E2E | 27b build / 35b verify |
| M2: Query + Authz | 2-3 days | 12-15 | Negative-authz suite passing | 27b build / 35b verify |
| M3: Identity | 2-3 days | 10-14 | All 4 auth methods in CI | 27b build / 35b verify |
| M4: Evidence | 3-4 days | 12-16 | Byte-identical export, DuckDB load | 27b build / 35b verify |
| M5: Delivery | 2-3 days | 8-10 | Full acceptance suite on ref hardware | 27b build / 35b verify |

Total: ~2-3 weeks agent-time, ~80 tasks.

### Provider Switching Protocol

Sergey's config stores both providers. Switching is explicit at phase boundaries:

```bash
# Switch to planning mode (35b)
hermes config set model.provider lmstudio && systemctl --user restart hermes-gateway
# Switch to execution mode (27b, reasoning OFF)
hermes config set model.provider p40 && systemctl --user restart hermes-gateway
```

Session restart is the clean context boundary — each phase starts fresh.

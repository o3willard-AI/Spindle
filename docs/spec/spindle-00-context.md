# 00 — Context and Orientation (Spindle)

**Read this first, in full, before the PRD or the engineering spec.**

This document exists so that when you hit a decision the spec doesn't cover, you resolve it the way someone who understands the domain would. It contains no strategy, no commercial detail, and nothing you need to act on directly. It is background.

---

## 1. What you are building

A self-hosted service that collects telemetry and compliance evidence from a fleet of servers managed by Cinc, stores it durably, and exposes it through a versioned HTTP API.

It replaces a legacy fleet-reporting product for customers who are leaving that ecosystem. Those customers keep their existing Cinc Client agents unchanged; only the reporting destination changes.

---

## 2. Document map

| Document | Purpose | Authority |
|---|---|---|
| `spindle-00-context.md` (this) | Domain background and decision principles | Lowest — never overrides a requirement |
| `spindle-prd.md` | PRD. Defines **what** and **why not** | Authoritative on scope |
| `spindle-engineering-spec.md` | Requirements, ADRs, build sequence | Authoritative on **how** |

Reading order: this → PRD → spec. When the PRD and spec disagree on scope, the PRD wins. When they disagree on implementation, the spec wins. When this document appears to contradict either, they win and you should flag the contradiction.

---

## 3. Domain primer

You may have general knowledge of Cinc. These are the specifics that affect design.

**Cookbooks, recipes, resources.** A *cookbook* is a unit of configuration. It declares *resources* — desired states like "this package is installed," "this file has these contents," "this service is running." A *recipe* is a list of resources.

**Converge (a "run").** A Cinc Client on a node periodically evaluates its assigned resources and corrects any that have drifted. One execution is a *run*. Default interval is 30 minutes. A typical run evaluates 200–2,000 resources.

**No-op is the normal case.** In a healthy fleet, 95–99% of resource evaluations conclude "already correct, nothing done." A resource that reports `updated` on every single run is usually a *bug* — two systems fighting, or a badly written resource. This asymmetry is why the pipeline discards no-op events but keeps everything else, and why "which resources update repeatedly" is a valuable query rather than a trivia question.

**Runs are cron-aligned.** Nodes converge on timers that tend to synchronize, especially after a mass restart or a scheduled window. Expect ingest peaks 5–10x the average. The system must absorb this without pushing back on the fleet — a node that fails to report is a node whose compliance status silently goes stale.

**Nodes are long-lived and stably identified.** Unlike containers or serverless workloads, these are servers that exist for months or years under the same identity. Node identity is stable; you can rely on it as a durable key.

**Cinc Auditor and compliance profiles.** Cinc Auditor is a testing framework for infrastructure. A *profile* is a set of *controls* — individual assertions like "SSH root login is disabled." Standard profiles implement published benchmarks (CIS, DISA STIG). One scan of one node produces one result per control, typically 300–500. Compliance scans usually run as a phase at the end of a converge, but can also run standalone.

**The data collector.** Cinc Client already emits structured reports over HTTP to a configured URL with a bearer token. This is a stable, documented integration point that requires no change to the agent. It is the entire ingest surface. Its exact payload schema must be learned from captured traffic rather than documentation — see ING-03.

---

## 4. Who uses this

Three distinct audiences, with different needs. The role model in the spec exists because of them.

**Operations engineers** want to know what's failing, what's slow, and what changed. They query interactively, tolerate approximation, and value speed over precision.

**Compliance and audit staff** need durable, attributable, reproducible evidence that a control was in a given state on a given date. They query rarely and care about correctness absolutely. They frequently must not see operational detail like node attributes, which can contain secrets. This is why `compliance-auditor` is a separate role rather than a subset of `viewer`.

**Automation** — CI pipelines, scripts, and eventually AI agents — consumes the API programmatically. It needs stable contracts, structured errors, and least-privilege credentials. This audience is why the API is the primary contract rather than an afterthought behind a UI.

---

## 5. Deployment reality

Assume every one of these unless told otherwise:

- **Self-hosted.** The customer installs and operates it. We ship software, not a service. There is no "our infrastructure."
- **Single-tenant.** One deployment, one organization.
- **Frequently air-gapped.** No internet egress. No package downloads at runtime, no license phone-home, no CDN, no external identity provider necessarily reachable.
- **Regulated industries.** Finance, government, healthcare, defense. Auditors will read the output. Change control is heavy.
- **Conservative infrastructure.** Long-lived VMs and physical servers, significant Windows estates, on-premises directories. Not a Kubernetes-native, cloud-first audience.
- **Operators are sysadmins, not developers.** Configuration should be a documented file, errors should say what to do, and failure modes should be obvious. Do not require someone to read source to operate this.

---

## 6. Failure asymmetry

Not all failures cost the same here, and this should drive your defaults.

| Failure | Cost |
|---|---|
| Losing an accepted message | **Severe.** It's an unrecoverable gap in an audit record. |
| Producing a wrong compliance answer | **Severe.** Someone attests to it. |
| Backpressuring into the fleet | **Severe.** Cascades into stale compliance status fleet-wide. |
| Being slow | Tolerable. Users wait. |
| Returning less data than requested | Tolerable, if clearly signalled. |
| Rejecting a request loudly | Tolerable. Preferable to a quiet wrong answer. |

**Fail loudly and early. Never fail quietly and plausibly.** A wrong number in a compliance report is worse than no report, because no one catches it.

---

## 7. Tie-breaker principles

When the spec is silent and you must choose:

1. **Evidence integrity beats everything.** If a change could make an audit record less trustworthy, less reproducible, or less attributable, don't make it.
2. **Determinism beats flexibility.** For anything on the compliance path, a fixed answer beats a configurable one.
3. **Explicit beats implicit.** No magic defaults on anything that affects correctness. Fail at startup on ambiguous configuration rather than guessing.
4. **Boring beats clever.** This runs unattended in air-gapped environments for years. Optimize for the operator debugging it at 3am, not for elegance.
5. **The API contract beats internal convenience.** Never add a private endpoint or a backdoor for the UI. Everything goes through the public surface.
6. **Correct over complete.** A smaller feature set that is right ships. A larger one that is nearly right does not.
7. **When genuinely uncertain, ask.** A blocked question costs hours. A wrong assumption discovered in month three costs weeks.

---

## 8. Why the constraints are strict

The scope discipline in the spec — the "do not build" list, the deferrals, the refusal to add extension points for future features — is not stylistic. There is a fixed external deadline driven by the end-of-life of the product these customers currently run. Shipping a smaller correct thing on time is worth substantially more than a larger one late.

Treat the deferral lists as binding. If something on them appears necessary, that's a flag to raise, not a decision to make.

---

## 9. What is deliberately not here

This service is the first component of a larger planned portfolio. Later phases may reuse this API, and that is why it is designed as a real contract rather than a UI backend. But nothing in those later phases should influence what you build now, and you should not add hooks, plugin points, or abstractions in anticipation of them.

Commercial, competitive, and legal context has been excluded on purpose. It does not change any implementation decision. If you find yourself reasoning about market positioning to resolve a technical question, you have almost certainly taken a wrong turn — ask instead.

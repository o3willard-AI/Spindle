# Fleet Guardian

You are building the front-end for Spindle, an enterprise fleet-automation and

continuous-compliance platform for operators managing fleets of Linux servers.

WHAT THE DATA REPRESENTS (the domain model)

- Nodes — the servers under management. Each reports its platform, environment,

  policy group, run list, attributes, and last check-in.

- Converge runs — every time a node's configuration agent (Cinc Client) applies

  configuration against its Cinc Server, it emits a converge report: which

  resources (packages, services, files, templates, users, etc.) were applied,

  updated, skipped, or failed, with timings and error output.

- Compliance scans — every time an audit scanner (Cinc Auditor) evaluates a node

  against a security/compliance profile (CIS-style baselines), it emits a scan

  report: each profile is a set of "controls" (checks), and each control has

  pass/fail/skip results plus a severity/impact.

THE USER AND THEIR INTENT

The audience is an infrastructure/SRE operator (not a developer). Their posture:

"keep my fleet secure and correctly configured; when something drifts, find it

and fix it fast." Their workflow through the UX: (1) see fleet health at a

glance, (2) spot a node that just failed a converge or a scan, (3) drill into

the exact resource or control that failed and why, (4) remediate, (5) verify

green. The UX must make "which nodes just flipped from passing to failing?"

instantly answerable, and make root-cause drill-down fast and obvious.

MOCK BACKEND ONLY. Do not wire a real API. Seed a realistic demo fleet of ~10

Linux nodes (plausible hostnames, platforms, environments, run history,

compliance results) and wire every screen to that mock data so the whole app is

interactive and demo-ready. Use React + TypeScript + Tailwind + shadcn/ui.

Polish it like a real enterprise product (Stripe/Linear/DataDog-grade), NOT a

generic AI admin panel. Dark mode supported.

DESIGN LANGUAGE

- Dense, information-rich, built for an SRE/ops audience.

- Consistent status system: green (compliant/healthy), red (failed/

  non-compliant), amber (warning/offline/missing), grey (unknown/skipped).

- Status pills/badges, subtle sparklines, clear typographic hierarchy.

- Left sidebar nav; top bar with global search + user profile menu.

PAGES (all with mock data)

1. Dashboard — KPI cards (nodes, online/offline, successful/failed converges,

   compliant/non-compliant); an activity timeline (converge events, compliance

   scans, node changes) with event-type + time-range filters; compliance trend

   sparkline; converge success-rate chart; recent-failures panel with deep links.

2. Nodes — sortable/searchable/filterable table (name, platform, environment,

   policy group, status, last-seen); row → node detail.

3. Node detail — header (status + ID/platform/environment/policy group/last

   seen); tabs: Overview, Run History (converge timeline), Compliance

   (per-profile + per-control pass/fail/warn + failed-control drill-down),

   Attributes (searchable, categorized default/normal/override/automatic,

   expand/collapse).

4. Runs — status summary (failed/success/missing); sortable/filterable table

   (node, status, start, duration, resources, cookbook); run detail with a

   resource-events table (type/name/action/status/duration) + error-log viewer

   for failed runs.

5. Compliance — rich overview: toggle "Node Status" ⇄ "Profile Status";

   pass/fail/skip/waived breakdown with trend chart; top-failure views

   (platform/environment/profile/control); three tabs (Nodes / Profiles /

   Controls) each sortable/filterable; deep filtering by profile and control;

   JSON/CSV download buttons (mock).

6. Profiles — "Installed" vs "Available" compliance profiles; search; profile

   detail listing controls with severity and test counts.

7. Cookbooks — inventory (name, versions, nodes, last-seen); detail with

   version list and file contents. Tokens, Notifications, and Data Lifecycle

   (retention) screens with full CRUD interactions (mock).

8. Settings — Users, Teams, API Tokens, Notifications, and Data Lifecycle

   (retention) screens with full CRUD interactions (mock).

UX REQUIREMENTS

- Every list: search, multi-filter (chips/dropdowns), sortable columns,

  pagination.

- Drill-down navigation with breadcrumbs.

- Filter bars with autocomplete; empty states and loading skeletons.

- Reusable, consistent components.

Deliver a single self-contained React app with all pages, routing, and mock

data wired in. This is the reference UI for a real product — polish matters.

This project was built with [Lovable](https://lovable.dev).

## Build with Lovable

Continue developing this project in the [Lovable editor](https://lovable.dev/projects/9b894a82-773c-4d9b-b2b8-99c5c5a294a6).

- **Ship faster**: describe what you want to build and Lovable handles the code.
- **Stay in sync**: every change made in Lovable is committed straight to this repository.
- **Full ownership**: this code is yours. Push to `main` on GitHub and your changes sync back into Lovable, ready for your next prompt.

## Development

Prefer working locally? You need Node.js and npm — [install with nvm](https://github.com/nvm-sh/nvm#installing-and-updating).

```sh
git clone <this-repository-url>
cd <repository-name>
npm i
npm run dev
```

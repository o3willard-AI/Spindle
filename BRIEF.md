# Spindle Progress Tracker

## M0 — Foundation ✅
10/10 complete (Sergey)

## M1 — Ingest to Storage ✅
26/26 complete

| Agent | Tasks | Tests |
|---|---|---|
| Mike | 17 | 63 server + 51 pipeline |
| Mark | 6 | 57 pipeline + 8 store |
| Sergey | 5 + M1-03 | 11 rawarchive |

## M2 — Query + Authorization 🏃
12/14 complete

| Task | Agent | Status |
|---|---|---|
| M2-01+02 Filter+Pagination | Mark | ✅ |
| M2-03 Nodes endpoint | Mark | ✅ |
| M2-04 Runs endpoint | Mike | ✅ |
| M2-05+07 Aggregates+Waivers | Mark | ✅ |
| M2-06 Compliance endpoints | Sergey | ✅ |
| M2-08 Cookbook+health | Mike | ✅ |
| M2-09 OpenAPI | Mark | ✅ |
| M2-10 Error envelope | Mike | ✅ |
| M2-11 Provenance markers | Mike | ✅ |
| M2-12 Auth scoping | Sergey | ✅ |
| M2-13 Role model | Sergey | ✅ |
| M2-14 Negative auth suite | Mike | 🏃 |
| Bug fixes (6 tests) | Sergey | 🏃 |

## M3 — Identity (14 tasks)
Not started.

## Known Issues
- 6 pre-existing test failures in nodes.rs + resource_events.rs (Mark → Sergey)
- No live PostgreSQL — all DB tests use in-memory stores
- No live Dex — auth endpoints stubbed

## Last Updated
2026-08-08

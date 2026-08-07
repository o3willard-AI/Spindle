# M1-07: C4 Partition Management

**Status:** M1-06 ✅ complete. Move to M1-07.

## M1-07: SQL partition management function
**Requirements:** STO-02
**What:** manage_partitions() function called by worker cron. Idempotently creates partitions for next 7 days, detaches partitions older than warm threshold (90d default). Use advisory lock for concurrency.

**Key points:**
- Create partitions for next 7 days from current date
- Detach (do not drop) partitions older than 90 days
- Advisory lock (pg_try_advisory_lock) for concurrency safety
- Idempotent — running twice creates no duplicates
- Place in migrations/003_partition_management/up.sql

**Verify:** Run twice → no duplicate partitions. Future date insert lands in correct partition.

**After completion:** Report to Hermes Command room.

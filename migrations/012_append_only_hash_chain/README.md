# Migration 012: Append-only Enforcement + Hash Chain

**Purpose:** Enforce immutability on evidence tables, enable corrections via inserts with `correction_of` FK, and link rows via deterministic SHA-256 hash chains.

**Requirements:** STO-05, STO-06 (M1-09)

**Evidence tables:** `runs`, `resource_events`, `control_results`, `compliance_reports`

## What was implemented

### 1. chain_tail table
Tracks the last hash for each evidence table — enables hash chain verification across table boundaries.

### 2. Hash columns added to evidence tables
- `prev_row_hash TEXT` — SHA-256 of the previous row's data
- `row_hash TEXT` — SHA-256 of this row's data
- `correction_of UUID` — FK pointing to original row for corrections

### 3. BEFORE INSERT trigger (`trg_set_prev_row_hash_and_hash`)
- Sets `prev_row_hash` from `chain_tail.last_hash`
- Computes `row_hash` via `compute_row_hash()` — deterministic SHA-256
- Updates `chain_tail` with new `row_hash`

### 4. BEFORE UPDATE/DELETE triggers (`trg_prevent_update`, `trg_prevent_delete`)
- **Rejects all UPDATE and DELETE** on evidence tables
- Error message: "UPDATE is forbidden — insert a corrected row with correction_of instead"
- Corrections: INSERT a new row with `correction_of` pointing to original

### 5. Hash chain verification (`verify_hash_chain(table_name)`)
- Iterates all rows in chronological order
- Checks each row's `prev_row_hash` matches previous row's `row_hash`
- Returns OK or MISMATCH detail

### 6. Hash chain reconciliation (`reconcile_hash_chain(table_name)`)
- Recomputes all hashes if migration disrupted the chain
- Resets `chain_tail` to genesis, then re-applies

### 7. Checkpoint signing placeholder (`checkpoint_sign(table_name)`)
- TODO for C9: signs `chain_tail.last_hash` with checkpoint signing key
- Currently logs the checkpoint and returns unsigned hash

### 8. Triggers attached to all evidence tables
- Uses conditional DDL (`DO $$ ... IF EXISTS ...`) so migration works even if some tables don't exist yet
- Each table gets 3 triggers: hash computation, UPDATE prevention, DELETE prevention

## Hash computation (deterministic)
- Uses `concat_ws('|', ...)` with explicit column ordering
- `COALESCE(..., 'NULL')` ensures consistent string representation
- Columns: id, node_id, run_id, status, start/end_time, counts, JSONB fields, schema_version, created_at, correction_of
- `row_hash` and `prev_row_hash` columns excluded from computation (prevents circular references)

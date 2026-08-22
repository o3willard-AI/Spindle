# Spindle CLI Reference

The `spindle` CLI (`spindle-cli`) is the operator interface for querying fleet
data, managing compliance waivers, and administering signing keys.

## Global Flags

| Flag | Short | Env Var | Default | Description |
|---|---|---|---|---|
| `--output <fmt>` | `-o` | — | `human` | Output format: `json` or `human` |
| `--json` | — | — | false | Shorthand for `--output json` |
| `--profile <name>` | — | `SPINDLE_PROFILE` | `default` | Config profile |
| `--config <path>` | — | `SPINDLE_CONFIG` | `~/.config/spindle/config.toml` | Config file path |
| `--server <url>` | — | `SPINDLE_SERVER` | `http://127.0.0.1:3000` | Server URL override |
| `--help` | `-h` | — | — | Print help |
| `--version` | `-V` | — | — | Print version |

## Subcommand Overview

```
spindle <subcommand> [options]

Subcommands:
  nodes           Node management
  runs            Run management
  compliance      Compliance reporting
  waivers         Waiver management
  cookbooks       Cookbook management
  resources       Resource event aggregates and drift detection
  health          System health check (exit 0 = healthy, exit 3 = unhealthy)
  health-metrics  System health metrics
  verify-archive  Verify an archive against a published keys.json URL
  metrics         System metrics
  migrate         Database migrations
  archive         Archive management
  tokens          Token reconciliation (operator)
  keys            Key management (operator)
  config          Configuration management
```

---

## config

Manage configuration profiles stored in the config file.

### `config init`

Interactive wizard to create a new config profile.

```bash
spindle config init
# Prompts for: API URL, token, output format
```

### `config show`

Display the current configuration.

```bash
spindle config show
```

**Output (human):**
```
Profile: default
API URL: http://127.0.0.1:3000
Token:   spindle-dev-token
Output:  human
```

### `config set <key> <value>`

Set a configuration value.

```bash
spindle config set api_url http://192.0.2.5:3000
spindle config set token my-secret-token
spindle config set output json
```

---

## nodes

Query fleet node inventory.

### `nodes list`

```bash
spindle nodes list --platform ubuntu
```

**Flags**: `--platform`, `--status`, `--search`

**Output (human):**
```
ID                                   Name            Platform  Status     Last Seen
3f9f50a9-54f7-5b20-909c-c6eb39dc7ba9 web-server-01   ubuntu    compliant  2026-08-13 10:05
5a1b3c2d-...                         db-server-01    ubuntu    failed     2026-08-13 09:30
```

### `nodes show <id>`

```bash
spindle nodes show 3f9f50a9-54f7-5b20-909c-c6eb39dc7ba9
```

**Output (human):**
```
Node: web-server-01 (3f9f50a9-...)
  Platform:     ubuntu 22.04
  Status:       compliant
  Environment:  production
  Project:      acme
  Policy:       web_server @ web
  First seen:   2026-08-01
  Last seen:    2026-08-13 10:05:30
```

### `nodes state <id>`

Show lean current state for a single node (faster than `show` for dashboards/scripts).

```bash
spindle nodes state 3f9f50a9-54f7-5b20-909c-c6eb39dc7ba9
```

---

## runs

Query run history.

### `runs list`

```bash
spindle runs list --node 3f9f50a9-54f7-5b20-909c-c6eb39dc7ba9 --limit 5
```

**Flags**: `--limit`, `--node` (node ID), `--status`, `--since` (RFC 3339 timestamp)

### `runs show <id>`

```bash
spindle runs show uuid-of-run
```

> **Note**: `<id>` is the DB row UUID (from `runs list` output), not the Cinc `run_id`.

### `runs resources <id>`

List resource events for a run.

```bash
spindle runs resources uuid-of-run
```

---

## compliance

Query compliance reports.

### `compliance reports`

```bash
spindle compliance reports --node web-server-01
```

**Flags**: `--node`, `--profile`, `--status` (pass, fail, warn)

### `compliance show <id>`

```bash
spindle compliance show report-uuid
```

### `compliance status`

```bash
spindle compliance status --node web-server-01
```

### `compliance controls`

```bash
spindle compliance controls --node web-server-01
```

### `compliance export`

```bash
spindle compliance export --node web-server-01
```

---

## waivers

Manage compliance waivers.

### `waivers list`

```bash
spindle waivers list
```

### `waivers create`

```bash
spindle waivers create \
  --control-id cis-1.1 \
  --profile-id cis-baseline \
  --justification "Compensating control in place" \
  --approver "security-team" \
  --days 30
```

**Flags**: `--control-id`, `--profile-id`, `--justification`, `--approver`, `--days`

**Output (human):**
```
Waiver created: abc123-uuid
  Control:    cis-1.1 (cis-baseline)
  Scope:      node / web-server-01
  Approver:   security-team
  Valid:      2026-08-13 → 2026-09-12
```

### `waivers update <id>`

```bash
spindle waivers update abc123-uuid --days 60
```

### `waivers delete <id>`

```bash
spindle waivers delete abc123-uuid
```

---

## cookbooks

List cookbook inventory.

### `cookbooks list`

```bash
spindle cookbooks list
```

### `cookbooks show <name>`

```bash
spindle cookbooks show apache2
```

**Output (human):**
```
Cookbook   Versions                       Nodes
apache2    8.1.0 (5), 8.0.2 (2)           7
nginx      2.3.0 (3)                      3
```

---

## resources

Resource event aggregates and drift detection.

### `resources aggregates`

```bash
spindle resources aggregates --group-by cookbook_name --window 24h
```

**Flags**: `--group-by` (`cookbook_name`, `resource_type`, `platform`), `--window`

### `resources drift`

```bash
spindle resources drift --window 24h --threshold 5 --node node-001
```

**Flags**: `--window`, `--threshold`, `--node`

---

## keys

Manage Ed25519 signing keys (local, not API-based).

### `keys generate`

Generate a new signing key and write it encrypted to disk.

```bash
spindle keys generate --path ~/.spindle/signing-key.aes --unlock "my-passphrase"
```

**Output:**
```
Key generated: ~/.spindle/signing-key.aes
Key ID: local:abc123def456...
```

### `keys rotate`

Rotate the signing key (re-encrypts with same unlock material).

```bash
spindle keys rotate --unlock "my-passphrase"
```

### `keys list`

List available signing keys.

```bash
SPINDLE_KEY_UNLOCK="my-passphrase" spindle keys list
```

---

## health

Check server health.

```bash
spindle health
```

**Output (human):**
```
Spindle server: http://127.0.0.1:3000
  Status: healthy
  Database:  healthy (3ms)
  Storage:   healthy (1ms)
  Dex:       healthy (12ms)
```

---

## Realistic Multi-Step Workflows

### Workflow 1: Initial Setup → Query Nodes

```bash
# 1. Initialize config
spindle config init
# Enter: http://127.0.0.1:3000, spindle-dev-token, human

# 2. Verify connectivity
spindle health

# 3. List all Ubuntu nodes
spindle nodes list --platform ubuntu

# 4. Get details on a specific node
spindle nodes show 3f9f50a9-54f7-5b20-909c-c6eb39dc7ba9

# 5. View recent runs for that node
spindle runs list --node 3f9f50a9-54f7-5b20-909c-c6eb39dc7ba9 --limit 5
```

### Workflow 2: Create and Manage a Waiver

```bash
# 1. Check current compliance status
spindle compliance reports --node web-server-01

# 2. Create a 30-day waiver for a failing control
spindle waivers create \
  --control-id cis-1.1 \
  --profile-id cis-baseline \
  --justification "Compensating control in place" \
  --approver "security-team" \
  --days 30

# 3. List all active waivers
spindle waivers list

# 4. Extend the waiver to 60 days
spindle waivers update waiver-abc123 --days 60

# 5. Revoke when no longer needed
spindle waivers delete waiver-abc123
```

### Workflow 3: Generate Key → Query → Export JSON

```bash
# 1. Generate a signing key
spindle keys generate --path ~/.spindle/signing-key.aes --unlock "secret"

# 2. Query nodes in JSON for piping
spindle nodes list --json | jq '.data[] | .name'

# 3. Check resource drift over 24h
spindle resources drift --window 24h --threshold 5
```

# Sergey — Spindle Project Brief

## Your Job
Build Spindle M0-01: the Corpus Capture Proxy. This is the first task in a 74-task plan to build a self-hosted Chef Automate replacement in Rust.

## Specs (already on your drive)
- `~/workspace/Spindle/docs/spec/spindle-00-context.md` — Domain primer (read first)
- `~/workspace/Spindle/docs/spec/spindle-prd.md` — What and why
- `~/workspace/Spindle/docs/spec/spindle-engineering-spec.md` — Requirements, ADRs
- `~/workspace/Spindle/PLANS.md` — 74-task breakdown

## Your Workspace
```
~/workspace/Spindle/
├── spindle-corpus-capture/src/
│   ├── main.rs
│   ├── config.rs
│   ├── proxy.rs
│   ├── recorder.rs
│   └── metadata.rs
├── Cargo.toml
├── docs/
│   ├── M0-01-corpus-capture-proxy.md  (your DESIGN.md)
│   └── spec/                           (spec docs)
└── PLANS.md
```

## M0-01 Task
Recording proxy between Chef Infra Client and a real Automate instance. Captures raw HTTP traffic to `/testdata/corpus/` with metadata (timestamp, content-type, client version). Must support ≥3 Chef client versions, ≥4 platforms, success/failure/partial runs, and compliance-phase runs. See ING-03 in the engineering spec.

## Execution
1. `cargo build` — get it compiling
2. `cargo test` — write tests as you go
3. `cargo run` — verify it works
4. `git add -A && git commit -m "M0-01: corpus capture proxy" && git push`

## GitHub Access
```
TOKEN=$(/usr/bin/keepassxc-cli show -p "$(cat ~/.hermes/secrets/keepass/.master-pw)" ~/.hermes/secrets/keepass/secrets.kdbx "General/Hephaestus GitHub PAT" | head -1)
git remote set-url origin "https://o3willard-AI:${TOKEN}@github.com/o3willard-AI/Spindle.git"
```

## Your Model
- **Provider**: p100
- **Endpoint**: http://198.51.100.68:1234/v1
- **Model**: Qwen3.6-27B Q4_K_M, 64K context, reasoning OFF
- **Speed**: 2× P100 GPUs on the same Proxmox host

## Rules
- No SSH to .68 — it's API-only
- cargo build/test/run all happen locally on YOUR filesystem
- Push commits to o3willard-AI/Spindle
- If stuck, respond via mesh or Telegram

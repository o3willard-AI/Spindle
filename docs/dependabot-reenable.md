# Re-enabling Dependabot (disabled 2026-08-18)

Dependabot version updates were disabled by deleting `.github/dependabot.yml`.
This documents why, and what to fix before re-enabling.

## Why it was disabled

The old config used a single `default` group with `patterns: ["*"]`, which lumped
**every** dependency update — including breaking major-version bumps — into one
giant PR (`chore(deps): bump the default group with 15 updates`). That one PR
bundled:

| Package | From -> To | Breaking? |
|---|---|---|
| axum | 0.7 -> 0.8 | yes |
| jsonwebtoken | 9 -> 11 | yes |
| thiserror | 1 -> 2 | yes |
| sha2 | 0.10 -> 0.11 | yes |
| governor | 0.7 -> 0.10 | yes |
| utoipa-swagger-ui | 8 -> 9 | yes |

These need **code changes** (renamed APIs, changed features), not just a version
string flip. The PR was accidentally merged (mislabeled as a version bump), broke
the build, and had to be force-pushed/reverted.

Dependabot also re-creates `dependabot/*` branches on origin, which is a
pre-scrub-history leak vector while the repo is private or going public.

## Before re-enabling

1. **Split the groups.** Do NOT use `patterns: ["*"]`. Isolate breaking (major)
   bumps from routine minor/patch updates so they're reviewed and tested
   separately. Example (verify against current Dependabot docs before use):

   ```yaml
   groups:
     breaking:
       update-types: ["major"]
     non-breaking:
       update-types: ["minor", "patch"]
   ```

2. **Verify every major bump** with `cargo test --workspace` and
   `cargo build --release`. Expect code changes for axum, jsonwebtoken, thiserror,
   sha2, governor, utoipa-swagger-ui.

3. **Keep the Rust toolchain aligned** with the build VMs (rust 1.97.1 on Alma).
   The Dependabot `rust 1.82 -> 1.97` bump is aligned and safe; a mismatch with
   the build VMs would produce different lock files.

4. **Delete/scrub `dependabot/*` branches** before flipping the repo public — they
   fork from pre-scrub history and carry old private IPs and credentials.

5. **Merge automation must use `owner:branch`** (e.g. `o3willard-AI:my-branch`) in
   the GitHub `pulls?head=` filter, not a bare branch name. A bare
   `chore/bump-v0.2.0` matched the wrong open PR and caused the accidental merge
   described above.

## The config that was removed (reference)

```yaml
version: 2
updates:
  - package-ecosystem: "cargo"
    directory: "/"
    schedule:
      interval: "weekly"
    open-pull-requests-limit: 5
    groups:
      default:
        patterns:
          - "*"
  - package-ecosystem: "github-actions"
    directory: "/"
    schedule:
      interval: "weekly"
    open-pull-requests-limit: 10
    groups:
      default:
        patterns:
          - "*"
  - package-ecosystem: "docker"
    directory: "/"
    schedule:
      interval: "weekly"
    open-pull-requests-limit: 5
    groups:
      default:
        patterns:
          - "*"
```

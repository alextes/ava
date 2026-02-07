---
schema_version: 9
id: ava-axji
title: design allow-always pattern UX
priority: P2
status: done
type: design
deps:
- ava-obhq
tags:
- tool
- approval
owner: null
created_at: 2026-02-07T17:17:11.538242Z
started_at: 2026-02-07T18:06:49.939584Z
completed_at: 2026-02-07T19:15:38.128044Z
---

the current "allow always" button in the telegram approval flow generates a pattern (`<executable> *`) and stores it, but the user has no visibility into what pattern gets saved. this needs better UX so users understand and can refine what they're approving.

## problem

when a user presses "allow always" for `cargo test -- --nocapture`, the system silently saves `cargo *` as an approval rule. the user doesn't know:
- what pattern was generated
- that it will now auto-approve ALL cargo subcommands (build, clean, publish, etc.)
- how to view or revoke the rule later

additionally, `generate_pattern()` and `matches_single()` don't handle:
- **env var prefixes**: `RUST_LOG=debug cargo test` → first token is `RUST_LOG=debug`, pattern becomes `RUST_LOG=debug *` (useless)
- **flags before subcommands**: `cargo --verbose test` → token[1] is `--verbose`, not the subcommand

## chosen design

### phase 1: env var stripping + transparency + rule management

**env var stripping** (prerequisite for everything else):
- add `strip_env_prefix()` that removes leading `KEY=VALUE` tokens (matching `^[A-Z_][A-Z0-9_]*=`)
- use in both `generate_pattern()` and `matches_single()` before token comparison
- `RUST_LOG=debug RUST_BACKTRACE=1 cargo test` → executable is `cargo`, pattern is `cargo *`

**show pattern in confirmation**:
- after pressing "allow always", edit the telegram message to include the saved pattern
- current: `-> approved (always)`
- new: `-> approved (always for: cargo *)`
- generate pattern at keyboard-build time, encode in callback data

**`/rules` command**:
- `/rules` — list all approval rules with numbered IDs
- `/rules delete <n>` — remove a rule by ID
- uses existing `list_approval_rules()` and `delete_approval_rule()` from the DB

### phase 2: pattern choices in keyboard

replace single "allow always" button with scope options when applicable:

```
command: cargo test -- --nocapture
[allow once] [deny]
[always: cargo test *] [always: cargo *]
```

**heuristic**: after stripping env vars, if token[1] exists and is alphabetic (doesn't start with `-`), offer both narrow and broad patterns. otherwise offer only the broad pattern.

examples:
- `cargo test -- --nocapture` → `[always: cargo test *]` + `[always: cargo *]`
- `git push origin main` → `[always: git push *]` + `[always: git *]`
- `ls -la /tmp` → `[always: ls *]` (only broad — `-la` is a flag)
- `cargo --verbose test` → `[always: cargo *]` (only broad — `--verbose` is a flag)
- `RUST_LOG=debug cargo test` → `[always: cargo test *]` + `[always: cargo *]` (env stripped)

**callback data format**: `exec:{nonce}:always:{url_encoded_pattern}` — pattern sent directly in the callback, no more `generate_pattern()` call on the approver side. telegram limits callback_data to 64 bytes; patterns are well within this.

### rejected: LLM-generated patterns

adds external API dependency, latency, token cost, and error handling complexity for marginal benefit over the deterministic heuristic.

## implementation issues

- ava-pc16 — strip env var prefixes in pattern generation and matching
- ava-3yg5 — show saved pattern in approval confirmation message
- ava-deyc — add /rules command for listing and deleting approval rules
- ava-dfiq — offer narrow/broad pattern choices in approval keyboard
- ava-gmi8 (pre-existing) — wire auto-approval rules into telegram approver
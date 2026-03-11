---
schema_version: 9
id: ava-apl2
title: sub-match approval rules for piped commands
priority: P2
status: done
deps:
- ava-dfiq
tags:
- approval
- tool
owner: null
created_at: 2026-03-09T21:20:38.043865Z
completed_at: 2026-03-11T10:44:42.416686Z
---

when a shell command contains pipes, match each segment independently against approval rules rather than the full compound command string.

## motivation

a command like `cargo build 2>&1 | tail -20` doesn't match the rule `cargo build *` because the pattern is matched against the full string. this means piped commands always require manual approval even when all segments are covered by existing rules.

## design

1. split the command on `|` into segments, trim whitespace from each
2. check each segment independently against existing approval rules
3. if ALL segments match existing rules → auto-approve, no prompt
4. if SOME segments match → only prompt for the uncovered segments
5. when suggesting new rules in the approval keyboard, only offer rules for uncovered segments (don't suggest rules already covered)

## examples

- `cargo build 2>&1 | tail -20` with rules `cargo build *` and `tail *` → auto-approved
- `cargo test 2>&1 | grep error` with rule `cargo test *` but no `grep *` → prompt only for `grep *`
- `foo | bar` with no rules → prompt as normal, suggest `foo *` and `bar *` as separate rule options

## notes

- `2>&1` style redirects should be stripped/ignored during segment matching (not a separate command)
- this composes well with ava-dfiq (narrow/broad pattern choices) — each uncovered segment gets its own narrow/broad options
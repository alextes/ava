---
schema_version: 9
id: ava-cd77
title: 'close pattern matching gaps: &, newlines, command substitution'
priority: P2
status: done
deps: []
tags:
- approval
- security
owner: null
created_at: 2026-03-11T11:11:22.192258Z
completed_at: 2026-03-11T11:19:30.304908Z
---

close three known gaps in split_subcommands and approval matching:

1. **single `&` (background)**: `cargo test & curl evil.com` — `&` not split on, whole string matches `cargo *`. add `&` as a delimiter.

2. **newlines**: `cargo test\nrm -rf /` — newlines not split on, whole string matches `cargo *`. add `\n` as a delimiter.

3. **command substitution / backticks**: `cargo test $(curl evil.com)` stays as one segment, matches `cargo *`. flag commands containing `$(`, `` ` `` as requiring explicit approval (never auto-approve, even if pattern matches). similar to claude code's approach.

## files
- `src/db/rules.rs` — update `split_subcommands` for `&` and `\n`, add `contains_command_substitution()` helper
- `src/approver.rs` — skip auto-approval when command contains substitution
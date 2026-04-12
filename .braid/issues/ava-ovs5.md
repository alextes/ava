---
schema_version: 9
id: ava-ovs5
title: extract history formatting from main.rs into display module
priority: P2
status: done
deps: []
tags:
- refactor
owner: null
created_at: 2026-02-11T11:29:50.390359Z
completed_at: 2026-03-24T08:16:05.346791Z
---

main.rs is 770 lines and mixes CLI orchestration with display formatting. ~170 lines of history display logic should move to a dedicated module.

### what to extract

- `truncate_str()` helper
- `truncate_json_strings()` helper
- `print_expanded_json()` helper
- the history rendering loop (HistoryMode enum, formatting per MessageContent variant)

### why

main.rs should focus on CLI argument parsing, command routing, and orchestration. display formatting is a separate concern that's easier to test and maintain in its own module.

### suggested target

`src/display.rs` or `src/history.rs` — a module that main.rs calls into for rendering conversation history.
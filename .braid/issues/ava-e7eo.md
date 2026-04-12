---
schema_version: 9
id: ava-e7eo
title: extract system prompt formatting from agent/mod.rs
priority: P2
status: done
deps: []
tags:
- refactor
owner: null
created_at: 2026-02-11T11:29:53.772937Z
completed_at: 2026-03-24T08:15:39.939316Z
---

agent/mod.rs is 1,417 lines — the largest file in the codebase. ~70 lines of pure formatting helpers can be extracted to reduce its size and improve readability.

### what to extract

- `format_character_traits()`
- `format_known_facts()`
- `format_recent_episodes()`
- `format_pending_tasks()`

these are stateless formatting functions that take data and return strings. they don't depend on Agent state.

### suggested target

`src/agent/prompt.rs` — a submodule focused on system prompt construction. the existing tests for these functions would move along with them.
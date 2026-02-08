---
schema_version: 9
id: ava-eauu
title: add tests for tool handler dispatch (remember/forget/recall)
priority: P2
status: done
deps: []
tags:
- test
- tool
owner: null
created_at: 2026-02-08T18:43:05.260394Z
started_at: 2026-02-08T18:43:51.076541Z
completed_at: 2026-02-08T18:47:56.482749Z
---

the remember, forget, and recall tool handlers in src/tool/mod.rs have no end-to-end tests exercising handle_tool_call(). the DB methods they call are tested, but input validation, error paths, and output formatting in the handler aren't.

## what to test

### remember handler
- valid fact with category + key
- valid episode (no category/key needed)
- valid character with key
- invalid kind value
- missing required fields (deserialization error)
- upsert behavior (fact + character return updated id)

### forget handler
- delete fact by kind + category + key
- delete character by kind + key
- delete episode by kind + id
- missing id for episode
- invalid kind
- not found returns "not found"

### recall handler
- search returns formatted results for each kind
- no results returns "no memories found"
- limit capping at 50
- default limit of 10

## files

- `src/tool/mod.rs` — add tests in the existing `#[cfg(test)]` module
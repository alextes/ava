---
schema_version: 9
id: ava-8a12
title: add tests for agent tool loop
priority: P2
status: done
deps: []
tags:
- test
- core
owner: null
created_at: 2026-02-08T18:43:14.80192Z
completed_at: 2026-02-08T18:47:56.495626Z
---

the multi-turn tool execution path in agent::process() (the loop that handles provider responses with tool_calls) is completely untested. no test sends a provider response containing tool_calls.

## what to test

- provider returns tool_calls → tool executed → result sent back → provider called again → final text response
- tool result message persistence (both assistant tool_use blocks and user tool_result blocks saved to DB)
- 5-round tool loop limit (error after exceeding)
- provider switching mid-conversation via switch_model tool
- model persistence when switching providers
- multiple tool calls in single response

## approach

use TestProvider with a stateful handler that returns tool_calls on first call, then a text response on second call. verify messages are persisted correctly.

## files

- `src/agent/mod.rs` — add tests in the existing `#[cfg(test)]` module
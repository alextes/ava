---
schema_version: 9
id: ava-feiu
title: move context usage injection out of tool result content blocks
priority: P1
status: done
deps: []
tags:
- observability
- session
owner: null
created_at: 2026-03-24T11:39:51.380331Z
completed_at: 2026-03-24T15:42:57.024951Z
---

context usage info (`[context: X% of window used ...]`) is currently appended as a `MessageContent::text` block inside the tool results message. this means it's mixed in with actual tool output.

## problem

if a tool returns structured content (JSON, code, etc.), appending a `[context: ...]` text block to the same message could interfere with the model's interpretation of the tool output. the model might treat the context line as part of the tool's response.

persisting the context line to history is fine — it was sent to the model, so it makes sense to see it in `ava history`.

## solution

move the context usage injection from being an extra block inside the tool results message to a **separate user message** sent immediately after the tool results. the anthropic API allows consecutive user messages, so this is valid.

### current flow (agent loop)

1. collect tool results into `tool_results: Vec<MessageContent>`
2. append `[context: ...]` text block to `tool_results`
3. persist `tool_results` as one user message
4. send to API

### proposed flow

1. collect tool results into `tool_results: Vec<MessageContent>`
2. persist `tool_results` as one user message
3. build a separate user message with just the `[context: ...]` text
4. persist that as its own user message
5. send both to API

## location

`src/agent/mod.rs` around line 399-408 — the `should_inject_context` block.

## acceptance criteria

- context usage info is never appended to tool result content blocks
- context info is sent as its own user message
- still persisted and visible in `ava history`
- same injection thresholds (first round, 60%, 80%+)

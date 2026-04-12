---
schema_version: 9
id: ava-mpet
title: design file attachments for data-heavy responses
priority: P2
status: done
type: design
deps: []
tags:
- tool
- telegram
owner: null
created_at: 2026-03-17T15:24:57.953326Z
started_at: 2026-03-17T15:25:03.111786Z
completed_at: 2026-03-17T20:18:33.988708Z
---

when ava produces data-heavy output (e.g. 1000 rows from a DB query), sending it inline as a telegram message is impractical — message size limits, formatting issues, context pollution. the agent should be able to write data to a file and attach it to the response.

## design

### what already works
- agent can write files via `text_editor` (create) or `exec` (shell commands like `sqlite3`)
- agent responses flow: agent loop → `OutboundMessage` → `send_response()` → telegram `sendMessage`

### what's missing
1. `OutboundMessage` is text-only — no way to carry file attachments
2. telegram bot has no `sendDocument` support
3. no tool for the agent to stage a file for attachment

### approach: attachments on ToolCallResult + OutboundMessage

**ToolCallResult** gains `attachments: Vec<PathBuf>`. the `attach_file` tool validates the file exists and returns it as an attachment on the result. the agent loop collects all attachments across tool rounds.

**OutboundMessage** gains `attachments: Vec<PathBuf>`. when the agent produces its final text response, accumulated attachments are included.

**send_response** in `queue.rs` sends each attachment via `sendDocument` after the text message.

this keeps tools channel-agnostic — they don't know about telegram, they just say "attach this file." the delivery layer handles the how.

### file validation
- `attach_file` tool checks: file exists, is a regular file, size < 50MB (telegram limit)
- files are read from disk at send time, so they must still exist when the response is delivered
- path validation reuses the same traversal checks as `text_editor`

### scope
- `src/message.rs` — add `attachments` to `OutboundMessage`
- `src/tool/mod.rs` — add `attachments` to `ToolCallResult`
- `src/tool/attach.rs` — new `attach_file` tool
- `src/telegram.rs` — add `send_document()` method (multipart upload)
- `src/queue.rs` — update `send_response` to handle attachments
- `src/agent/mod.rs` — collect attachments from tool results, pass to OutboundMessage

### planned implementation issues
1. add `attachments` field to `ToolCallResult` and `OutboundMessage`
2. add `send_document` to `TelegramBot` (multipart file upload)
3. add `attach_file` tool with path validation
4. wire attachments through agent loop and `send_response`

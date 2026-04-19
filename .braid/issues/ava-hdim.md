---
schema_version: 9
id: ava-hdim
title: add send_photo tool for inline telegram photos
priority: P2
status: done
deps: []
tags:
- tool
- telegram
owner: null
created_at: 2026-04-19T05:34:06.57576Z
started_at: 2026-04-19T05:34:09.11513Z
completed_at: 2026-04-19T06:09:02.052063Z
---

the `send_file` tool (src/tool/send_file.rs) sends attachments via telegram's `sendDocument`, which delivers images as downloadable files rather than inline previews. add a `send_photo` path that uses telegram's `sendPhoto` endpoint so images render inline.

scope:

1. add `TelegramBot::send_photo` in src/telegram.rs using multipart to `sendPhoto` (analogous to the existing send_document). return `Result<i64, Error>` (message_id).
2. either (a) extend send_file to detect image mime types and route to sendPhoto, or (b) add a sibling `send_photo` tool with its own ToolDefinition. leaning toward (b) — explicit tool, clearer affordance for the model, simpler control flow.
3. wire the attachment dispatch in the agent loop / send_response so a photo attachment variant goes through send_photo instead of send_document. may require a small enum on FileAttachment (document vs photo).
4. tests and cargo fmt/clippy.

open questions to resolve during planning:
- should we just add a `kind` field to FileAttachment, or introduce a separate PhotoAttachment?
- caption behavior is the same on both endpoints; nothing special needed there.
- telegram photo size limits (10MB via bot API) — add validation or let it fail naturally?
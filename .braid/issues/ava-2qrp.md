---
schema_version: 9
id: ava-2qrp
title: fix compaction not persisting to DB across turns
priority: P2
status: done
deps: []
tags:
- core
- session
owner: null
created_at: 2026-04-12T17:07:09.794971156Z
started_at: 2026-04-12T17:08:22.448084Z
completed_at: 2026-04-12T17:30:31.889266Z
---

compaction works in-memory for the current turn but is ineffective across turns because load_messages() reloads all messages from DB on every new request.

## bug

in agent/mod.rs, compact_messages() returns a compacted Vec<Message> and a summary string. the compacted messages replace the in-memory slice for that turn, and the summary is persisted to the session via set_session_summary(). however, the original messages are never deleted from the DB.

on the next turn, load_messages() loads all messages again — including everything that was compacted — so the context grows unbounded and compaction has zero lasting effect.

## fix

after a successful compaction, persist the compacted state back to the DB:
1. delete all messages for the session that predate the recent slice
2. insert a synthetic summary message in their place
3. load_messages() on the next turn then returns only [summary + recent]

alternatively, store a compaction cursor (message id) on the session and have load_messages() automatically inject the summary and skip older messages.

## impact

repeated calls to compact_context have no effect on token usage — each turn reloads the full history regardless.
---
schema_version: 9
id: ava-sjau
title: add per-chat message ring buffer in telegram producer
priority: P1
status: done
deps:
- ava-fzw4
tags:
- telegram
owner: null
created_at: 2026-04-12T09:57:30.648671Z
started_at: 2026-04-12T11:23:35.888137Z
completed_at: 2026-04-12T11:26:41.33951Z
---

maintain a ring buffer of recent messages per chat_id in the telegram producer. this buffer serves two purposes: (1) providing conversational context when the bot is mentioned in a group, and (2) backing the channel_history tool.

## scope

- in `telegram_producer` (src/commands/start.rs), add a `HashMap<i64, VecDeque<BufferedMessage>>` keyed by chat_id
- `BufferedMessage`: timestamp, user display name, user_id, text, message_id
- every incoming text message (regardless of whether it passes the mention filter) gets added to the buffer
- configurable max entries per chat (default: 50) and max age (default: 30 minutes)
- prune stale entries on each insertion
- the buffer must be accessible from the agent (for the channel_history tool) — either pass it through the queue as an Arc<Mutex<...>> or attach a snapshot to the QueuedMessage

## acceptance criteria

- messages are buffered per chat_id
- buffer respects size and age limits
- buffer is populated for all messages, not just those that trigger the agent
- buffer state is accessible from outside the producer (for tool use)
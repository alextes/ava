---
schema_version: 9
id: ava-rv0i
title: design unified message queue and agent loop
priority: P2
status: done
type: design
deps: []
tags:
- core
- session
owner: null
created_at: 2026-02-08T16:00:24.238029Z
completed_at: 2026-02-08T17:57:23.368417Z
---

the current architecture spawns independent tokio tasks per inbound message (telegram) or runs one-shot (CLI). all tasks share a single active session. this causes a real bug: if two messages arrive close together, both load conversation history at ~the same time, process independently, and append results — neither sees the other's response, producing interleaved turns that break conversation coherence.

## proposed architecture

a global inbound message queue fed by all channels. a single agent loop consumes from the queue:

1. pop message(s) from queue
2. process turn (load history, call provider, execute tools, persist)
3. send response back to originating channel
4. before waiting for next message, check queue — if messages arrived during processing, batch them into the next turn (like a human: if you get two questions while thinking, address both)

this batching naturally handles the "user sends 3 messages before AI replies" case — all queued messages become part of the next user turn.

## concurrency safety

- channels only push to the queue (no direct DB writes for conversation)
- agent loop is the single writer for conversation history
- eliminates race conditions between concurrent tasks
- SQLite remains fine since there's only one writer

## multi-channel implications

- queue entries carry channel metadata (where to send the response)
- a single conversation can span channels (already true today)
- responses route back to the originating channel

## cron/heartbeat integration point

scheduled tasks and heartbeats would also push "synthetic" messages into the queue, making them first-class participants in the conversation without special-casing.

## questions

- should the queue be in-memory (tokio::mpsc) or persisted (SQLite table)?
- how to handle backpressure if the agent is slow and messages pile up?
- should batched messages be presented as separate user messages or concatenated?
- timeout for queue draining before starting next turn?

## output

- queue data structure and API
- agent loop pseudocode
- channel → queue → agent → channel flow diagram
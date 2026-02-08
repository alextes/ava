---
schema_version: 9
id: ava-lzbb
title: restructure binary around unified ava run command
priority: P2
status: open
deps:
- ava-rv0i
tags:
- core
owner: null
created_at: 2026-02-08T16:00:37.573446Z
---

replace channel-specific binary commands (`ava telegram`) with a unified `ava run` that starts all configured channels and drives a single agent loop.

## current state

- `ava message <text>` — one-shot CLI, creates its own agent
- `ava telegram` — long-running polling loop, spawns independent tasks per message

adding another channel (e.g. whatsapp) would mean adding `ava whatsapp` — a separate process with its own event loop and its own concurrency problems.

## proposed change

```
ava run
```

starts a single long-running process that:
1. spawns all configured channel listeners (telegram poller, future whatsapp webhook, etc.)
2. all channels push inbound messages into the shared queue (from ava-rv0i)
3. single agent loop processes the queue
4. responses route back to originating channels

## channel configuration

channels are enabled based on which env vars are set:
- `TELOXIDE_TOKEN` → start telegram listener
- future: `WHATSAPP_TOKEN` → start whatsapp listener
- no tokens → log warning, run with no channels (useful for cron-only mode)

## CLI message command

`ava message <text>` can remain as a convenience for one-shot usage. it either:
- pushes to the queue and waits for response (if `ava run` is already running), or
- runs its own ephemeral agent loop (current behavior, for quick testing)

## acceptance criteria

- `ava run` starts all configured channels
- `ava telegram` removed or aliased to `ava run`
- single agent loop, no per-message task spawning
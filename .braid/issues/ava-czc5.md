---
schema_version: 9
id: ava-czc5
title: add channel_history tool
priority: P1
status: done
deps:
- ava-sjau
tags:
- telegram
owner: null
created_at: 2026-04-12T09:57:48.123434Z
started_at: 2026-04-12T11:41:30.332931Z
completed_at: 2026-04-12T11:46:45.689929Z
---

give the agent a tool to query ring buffers from any channel the bot is monitoring. this enables cross-channel awareness on demand.

## scope

- new tool in src/tool/: channel_history
- two modes:
  - **list channels:** returns all channels with active buffers — chat_id, title (if group), type (private/group/supergroup), message count, most recent message timestamp
  - **get history for channel:** given a chat_id, returns the ring buffer contents as formatted text (timestamp, user, message)
- the tool needs access to the shared ring buffer state (Arc<Mutex<HashMap<i64, VecDeque<BufferedMessage>>>>)
- for ava (single DM): lists one channel. for ren (multi-channel): lists all monitored channels
- format output as readable text, not JSON — the agent will reason about it, not parse it

## acceptance criteria

- tool is registered and available to the agent
- listing channels shows all chats with buffered messages
- getting history for a specific channel returns formatted recent messages
- returns helpful message when no channels have buffers or requested channel has no history
- works for both single-channel (ava) and multi-channel (ren) deployments
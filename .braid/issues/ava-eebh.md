---
schema_version: 9
id: ava-eebh
title: group chat response mode — mention-only default + intelligent response
priority: P1
status: done
type: design
deps: []
tags:
- telegram
owner: null
created_at: 2026-04-12T09:24:51.818044Z
started_at: 2026-04-12T09:25:58.880497Z
completed_at: 2026-04-12T09:57:52.675144Z
---

ava currently responds to every message from whitelisted users regardless of chat type. before dropping ren into team group chats, we need response gating so the bot isn't replying to every message in a channel.

## problem

- `telegram.rs` only captures `chat_id`, not chat type (private/group/supergroup)
- no concept of "should i respond to this message?" — every inbound message triggers the full agent pipeline
- in DMs this is correct; in group chats it's unacceptable

## goals

1. **mention-only mode (table stakes):** in group chats, only respond when @mentioned, replied to, or explicitly addressed by name. this must ship before ren enters any team chat.
2. **intelligent response mode (stretch):** optionally allow the bot to chime in when it detects an opportunity to contribute — using a hybrid heuristic + LLM classifier approach.

## research findings

### standard behavior in existing bots

every major bot (slack chatgpt, discord bots, copilot) defaults to mention-only in groups. thread continuation is the main exception — once pulled into a thread, the bot stays active.

### proposed response tiers

- **always respond:** @mention, DM, reply-to-bot-message, bot name in message text
- **thread stickiness:** once in a thread, stay active until it goes quiet (~5-10 min timeout)
- **LLM-as-judge for ambiguous cases:** cheap/fast model classifies "should i respond?" — costs ~0.1-0.5 cents per classification vs full agent cost
- **bias toward silence:** false negatives (missing a message) are far less costly than false positives (annoying the channel)

### key code areas

- `src/telegram.rs` — `Chat` struct only has `id`, needs `type` field
- `src/commands/start.rs:244-323` — telegram producer loop, where filtering would go
- `src/message.rs:162-175` — `ChannelKind` enum, no group/DM distinction
- `src/agent/mod.rs` — agent loop processes every message unconditionally

### per-channel config

allow configuring channels as "mention-only" (default for groups), "active" (bot monitors and may chime in), or "off".

## design

### context

ava (personal assistant) and ren (team bot) are both instances of the same harness but operate differently:
- **ava:** DM-only, every message gets a response, single channel (alex). multiple input modalities (telegram, voice) but always 1:1.
- **ren:** multi-channel, mention-gated in groups, should feel like one entity across channels.

the harness needs to support both modes cleanly.

### approach: producer-side filtering with message buffer

filtering happens in `telegram_producer` (before the queue), not in the agent. this avoids paying agent/API costs for every message in every channel.

**how it works:**
1. parse `chat.type` from telegram API to distinguish private vs group/supergroup
2. call `getMe` at startup to learn the bot's own username
3. maintain a per-chat ring buffer of recent messages (last N messages or last M minutes)
4. for private chats: always enqueue (current behavior preserved)
5. for group/supergroup chats: only enqueue if the message triggers a mention heuristic:
   - @mention (telegram entities with `type: "mention"` matching bot username)
   - reply-to a bot message (`reply_to_message.from.id == bot_id`)
   - bot name appears in message text (case-insensitive substring)
6. when a message passes the filter, prepend the channel's ring buffer as conversation context
7. messages that don't pass the filter still get added to the ring buffer (so context is available when the bot is eventually mentioned)

**cross-channel context via tool:**
- a `channel_history` tool lets the agent pull ring buffers from other channels on demand
- the tool lists which channels have active buffers (with chat title and message count)
- in ava's case this returns a single channel; in ren's case it returns all monitored channels
- this is a band-aid for tier 1 — tier 2 will do intelligent cross-channel context stitching

### scope

- `src/telegram.rs` — add fields to `Chat` (type), `Message` (reply_to_message, entities), add `getMe` call, add `User.is_bot`/`User.username`
- `src/commands/start.rs` — call `getMe` at startup, pass bot identity to producer, implement ring buffer and mention filter in `telegram_producer`
- `src/message.rs` — extend `InboundMessage` with chat context (chat type, chat title, buffered history)
- `src/tool/` — new `channel_history` tool
- `src/queue.rs` — extend `QueuedMessage` to carry chat context

### what this design does NOT cover (deferred to tier 2)

- LLM-as-judge classification for ambiguous messages
- thread stickiness / reply chain tracking
- intelligent cross-channel context awareness
- per-channel configuration (mention-only vs active vs off)

### planned implementation issues

**tier 1 — mention-only with context buffer:**

1. **parse chat type, bot identity, and reply-to from telegram API** — extend telegram types, add `getMe` endpoint
2. **design issue: multi-channel awareness in the harness** — figure out what telegram gives us for channel metadata (titles, types, member lists), how to track which channels the bot is in, and how this feeds into the agent's world model
3. **add per-chat message ring buffer in telegram producer** — buffer recent messages per chat_id, configurable size/TTL
4. **implement mention-only filter in telegram producer** — for group chats, only enqueue on @mention / reply-to-bot / name-in-text. inject buffer as context. DMs pass through unchanged
5. **add `channel_history` tool** — lets agent query ring buffers from other channels. lists available channels with metadata. returns recent messages for a requested channel

issues 1-2 can be worked in parallel. 3 depends on 1. 4 depends on 1+3. 5 depends on 3.
---
schema_version: 9
id: ava-h9es
title: implement mention-only filter in telegram producer
priority: P1
status: done
deps:
- ava-fzw4
- ava-sjau
tags:
- telegram
owner: null
created_at: 2026-04-12T09:57:42.775381Z
started_at: 2026-04-12T11:30:15.120434Z
completed_at: 2026-04-12T11:33:47.534944Z
---

add the core mention-only filter to the telegram producer. group/supergroup messages only reach the agent when the bot is explicitly addressed. DMs are unchanged.

## scope

- in `telegram_producer`, after adding a message to the ring buffer, check whether it should be enqueued:
  - **private chats:** always enqueue (current behavior)
  - **group/supergroup chats:** only enqueue if:
    - message entities contain a mention matching the bot's username (from getMe)
    - message is a reply to one of the bot's messages (reply_to_message.from.id == bot_id)
    - message text contains the bot's display name (case-insensitive substring, e.g. "hey ren")
- when a message passes the filter, prepend the channel's ring buffer snapshot as context in the InboundMessage so the agent understands the surrounding conversation
- extend InboundMessage or QueuedMessage to carry: chat_type, chat_title, buffered_context
- log skipped messages at debug level (for observability without noise)
- strip the @mention from the message text before sending to the agent (so "@ ren what's up" becomes "what's up")

## acceptance criteria

- DMs work exactly as before (no regression)
- group messages without mention/reply/name are silently buffered but not processed
- group messages with @mention, reply-to-bot, or name-in-text are processed with buffer context
- bot username is detected from getMe, not hardcoded
- bot display name detection is configurable via env var (TELEGRAM_BOT_NAME) as fallback
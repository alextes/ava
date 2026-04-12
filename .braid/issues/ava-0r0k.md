---
schema_version: 9
id: ava-0r0k
title: implement pre-agent authorization check in telegram producer
priority: P1
status: done
deps:
- ava-788p
tags:
- telegram
owner: null
created_at: 2026-04-12T10:51:25.823272Z
started_at: 2026-04-12T10:57:49.809328Z
completed_at: 2026-04-12T10:59:24.472367Z
---

replace the current in-memory allowed_ids check with DB-backed authorization. must run before ring buffer and queue — no LLM sees rejected messages.

## scope

- in telegram_producer, replace the current allowed_ids.contains() check (start.rs:300-305) with DB-backed auth:
  - **private chat (chat_type == \"private\"):** check is_user_allowed(user_id). if rejected, send a generic message: \"DM not available for this user.\" and continue. do not enqueue.
  - **group/supergroup:** check is_chat_allowed(chat_id). if not whitelisted, silently skip — no message, no enqueue, no buffer.
- the rejection message must be generic and static — no user input reflected back, no agent involvement
- remove allowed_ids Vec from telegram_producer args — it now queries DB directly
- the DB (Arc<Database>) needs to be passed to telegram_producer

## acceptance criteria

- DMs from whitelisted users work as before
- DMs from non-whitelisted users get a static rejection message, nothing hits the agent
- messages in whitelisted group chats are processed
- messages in non-whitelisted group chats are silently dropped
- no regression in callback_query handling
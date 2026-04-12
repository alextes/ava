---
schema_version: 9
id: ava-fzw4
title: parse chat type, bot identity, and reply-to from telegram API
priority: P1
status: done
deps: []
tags:
- telegram
owner: null
created_at: 2026-04-12T09:57:14.426861Z
started_at: 2026-04-12T10:04:02.542622Z
completed_at: 2026-04-12T10:06:26.681145Z
---

extend telegram API types and add getMe endpoint so the harness knows its own identity and can distinguish chat types.

## scope

- `src/telegram.rs`:
  - `Chat`: add `#[serde(rename = "type")] pub chat_type: Option<String>` (values: "private", "group", "supergroup", "channel")
  - `Chat`: add `pub title: Option<String>` (group chats have titles, DMs don't)
  - `Message`: add `pub reply_to_message: Option<Box<Message>>` 
  - `Message`: add `pub entities: Option<Vec<MessageEntity>>` 
  - add `MessageEntity` struct: `{ type: String, offset: i64, length: i64, user: Option<User> }`
  - `User`: add `pub username: Option<String>` and `pub is_bot: Option<bool>`
  - add `TelegramBot::get_me()` method — calls the `getMe` API, returns the bot's `User` (needed to know our own user_id and username)

## acceptance criteria

- `getMe` works and returns bot user_id + username
- `Chat` deserializes `type` and `title` from telegram JSON
- `Message` deserializes `reply_to_message` and `entities`
- `User` deserializes `username` and `is_bot`
- existing functionality (send_message, get_updates, etc.) unchanged
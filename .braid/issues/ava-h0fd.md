---
schema_version: 9
id: ava-h0fd
title: add channels registry table and my_chat_member handling
priority: P1
status: done
deps: []
tags:
- telegram
owner: null
created_at: 2026-04-12T10:51:33.697264Z
started_at: 2026-04-12T11:01:01.045474Z
completed_at: 2026-04-12T11:16:22.535432Z
---

track which chats the bot is in via a DB table, populated by telegram membership events and first-message upsert.

## scope

- new migration adding channels table:
  ```sql
  CREATE TABLE channels (
      chat_id INTEGER PRIMARY KEY,
      chat_type TEXT NOT NULL,
      title TEXT,
      added_at TEXT NOT NULL DEFAULT (datetime('now')),
      last_seen_at TEXT NOT NULL DEFAULT (datetime('now'))
  );
  ```
- Database methods:
  - upsert_channel(chat_id, chat_type, title) — insert or update last_seen_at + metadata
  - remove_channel(chat_id)
  - list_channels() -> Vec<ChannelInfo> (chat_id, chat_type, title, last_seen_at)
- expand allowed_updates in get_updates to include \"my_chat_member\"
- in telegram_producer, handle my_chat_member updates:
  - bot added (status → member/administrator): call getChat for metadata, upsert into channels
  - bot removed (status → left/kicked): remove from channels
- on each regular message from a chat: upsert channel with latest metadata and bump last_seen_at
- add ChatMemberUpdated, ChatMember structs to telegram.rs for deserialization
- add Update.my_chat_member field

## acceptance criteria

- channels table populated when bot receives messages
- my_chat_member events trigger insert/remove
- getChat called on bot-added events to fetch title and type
- list_channels returns accurate data
- existing message and callback_query handling unchanged
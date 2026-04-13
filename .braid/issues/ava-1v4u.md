---
schema_version: 9
id: ava-1v4u
title: send_message returns message_id
priority: P2
status: open
deps:
- ava-rpr9
owner: null
created_at: 2026-04-13T10:20:56.027741Z
---

change TelegramBot::send_message return type from Result<(), Error> to Result<i64, Error> (message_id). needed so we can later edit/delete the status message. update all call sites. also add delete_message method (sendMessage already returns a Message object with message_id — just deserialize it like send_message_with_keyboard does via SentMessage).
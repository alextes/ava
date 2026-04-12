---
schema_version: 9
id: ava-7zcb
title: add sendVoice to TelegramBot (multipart voice message upload)
priority: P2
status: done
deps: []
tags:
- telegram
- voice
owner: null
created_at: 2026-03-24T16:32:23.393975Z
started_at: 2026-03-25T12:49:55.036977Z
completed_at: 2026-03-25T12:52:17.597143Z
---

add `send_voice` method to `TelegramBot` that uploads OGG Opus audio bytes via the telegram `sendVoice` API. this is the transport layer for the speak tool.

### implementation

- add `send_voice(chat_id, ogg_bytes)` to `TelegramBot` in `src/telegram.rs`
- use `reqwest` multipart form: `chat_id` field + `voice` file part (bytes, filename `voice.ogg`, content-type `audio/ogg`)
- return the message ID on success
- telegram requires OGG Opus format for voice messages (displayed with inline waveform)

### test plan

- unit: multipart form is constructed correctly
- integration: send a voice message to a test chat (manual)

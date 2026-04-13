---
schema_version: 9
id: ava-l3dy
title: typing indicator during agent processing
priority: P2
status: done
deps: []
owner: null
created_at: 2026-04-13T10:06:54.105879Z
started_at: 2026-04-13T10:07:17.821092Z
completed_at: 2026-04-13T10:09:22.928901Z
---

add sendChatAction("typing") while the agent is processing. add send_chat_action to TelegramBot, spawn a background task in agent_loop that re-sends typing every 5s, cancel when process() returns.
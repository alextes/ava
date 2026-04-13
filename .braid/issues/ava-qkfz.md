---
schema_version: 9
id: ava-qkfz
title: status message lifecycle in agent_loop
priority: P2
status: open
deps:
- ava-0i2i
owner: null
created_at: 2026-04-13T10:21:08.103746Z
---

in commands/start.rs agent_loop: create mpsc channel, spawn receiver task that maps Progress events to telegram status messages. receiver sends initial status message on Thinking, edits on ToolRound/Compacting, deletes on channel close. after receiver completes (await), send the final response. replace the current typing indicator loop with this. add delete_message to TelegramBot.
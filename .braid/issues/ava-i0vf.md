---
schema_version: 9
id: ava-i0vf
title: whitelist telegram user IDs
priority: P1
status: done
deps: []
tags:
- telegram
owner: null
created_at: 2026-02-04T21:27:45.684986Z
completed_at: 2026-02-04T21:31:46.130373Z
---

only respond to messages from whitelisted telegram user IDs. reject messages from unknown users.

- add TELEGRAM_ALLOWED_IDS env var (comma-separated list)
- check user ID before processing messages
- silently ignore or send "unauthorized" for unknown users
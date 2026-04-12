---
schema_version: 9
id: ava-prx4
title: inject context usage into system prompt for agent self-awareness
priority: P2
status: done
deps:
- ava-wo9p
- ava-oh2z
tags:
- session
- observability
owner: null
created_at: 2026-03-15T22:20:59.723852Z
started_at: 2026-03-15T22:35:35.841414Z
completed_at: 2026-03-15T22:43:16.590054Z
---

inject a context usage section into the system prompt so the agent knows its own usage. append after existing tool budget section: '## context usage\nyou are currently using approximately 42% of your context window (84000/200000 tokens).\ncompaction will trigger at 80%. if context is full, suggest starting a new session.'\nomit section on first call when no data is available.
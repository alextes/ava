---
schema_version: 9
id: ava-vkoh
title: show context usage in ava status CLI
priority: P2
status: done
deps:
- ava-fmgy
- ava-oh2z
tags:
- session
- observability
owner: null
created_at: 2026-03-15T22:20:59.096334Z
started_at: 2026-03-15T22:34:23.747626Z
completed_at: 2026-03-15T22:35:30.937463Z
---

extend ava status CLI to read and display context usage and model. format: 'context: 42% (84000/200000 tokens)\nmodel: anthropic/claude-sonnet-4-5'. show 'context: unknown' when no usage data available yet.
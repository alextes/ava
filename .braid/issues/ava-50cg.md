---
schema_version: 9
id: ava-50cg
title: add current date/time to system prompt
priority: P1
status: done
deps: []
tags:
- core
- agent
owner: null
created_at: 2026-02-10T08:04:23.225336Z
completed_at: 2026-02-10T08:04:25.062227Z
---

the model has no awareness of 'now' — it scheduled a cron job for its training cutoff date instead of the actual current date. fix: include current UTC date/time in the system prompt, regenerated on every process() call.
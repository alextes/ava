---
schema_version: 9
id: ava-qtcv
title: design scheduled tasks
priority: P2
status: open
type: design
deps:
- ava-rv0i
tags:
- core
owner: null
created_at: 2026-02-04T21:37:12.985608Z
---

recurring and one-time automated task execution.

## aidaemon approach
- scheduler tool for agent to create tasks
- natural language parsing: "daily at 9am", "every 5m", "in 2h"
- standard 5-field cron expressions
- SQLite persistence
- trusted vs untrusted execution context
- missed tasks fire on recovery

## questions to consider
- natural language parsing library vs hand-rolled?
- should agent be able to create its own scheduled tasks?
- task output - where does it go? telegram notification?
- task failure handling and retry logic?
- timezone handling?
- resource limits for scheduled tasks?

## output
- storage schema
- scheduling engine design
- task execution context
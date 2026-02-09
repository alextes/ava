---
schema_version: 9
id: ava-j06f
title: add scheduler-driven task board check
priority: P3
status: open
deps:
- ava-g3o2
tags:
- core
- scheduler
owner: null
created_at: 2026-02-09T17:43:27.897935Z
---

bake a pending-tasks check into the existing scheduler loop (src/scheduler.rs). when the agent is idle and has pending tasks, push a synthetic message into the queue prompting it to review its backlog.

## context

design: ava-g3o2 (phase 2)

the scheduler already ticks every 60s checking for due cron schedules. this adds a second check in the same loop for pending tasks.

**important**: this must NOT be a cron entry — the agent could cancel it. it's built-in scheduler behavior the agent cannot disable.

## implementation

in the scheduler loop (src/scheduler.rs), after checking due_schedules:
1. query db.pending_task_titles()
2. if non-empty and enough time has passed since last task-board nudge (configurable interval, e.g. 30 min):
   - push synthetic message: 'you have N pending tasks. review your task list and make progress where possible.'
   - track last_nudge_at to avoid spamming

## configuration

- task board check interval (env var or config, default 30 min)
- active hours window (skip checks outside configured hours)
- both live in config/env, not in the schedules table
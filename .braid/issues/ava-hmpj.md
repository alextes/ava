---
schema_version: 9
id: ava-hmpj
title: design cron and heartbeat integration with message queue
priority: P2
status: open
type: design
deps:
- ava-rv0i
tags:
- core
owner: null
created_at: 2026-02-08T16:00:51.229416Z
---

design how cron jobs and periodic heartbeats integrate with the message queue architecture (ava-rv0i).

## motivation

ava needs periodic wake-ups for:
- scheduled tasks (checking triggers, running recurring jobs)
- heartbeats (proactive check-ins, monitoring)
- time-based reminders

these must coexist with active user conversations without interference.

## key design question

how do scheduled/periodic events enter the conversation?

option A: synthetic messages pushed to the queue
- heartbeat pushes a system-like message: "heartbeat: check scheduled tasks"
- agent processes it like any other turn
- natural batching: if user messages are also queued, they're processed together
- simple, uses existing infrastructure

option B: separate execution context
- scheduled tasks run in their own "session" or context
- results that need user attention are pushed as messages
- more isolation but more complexity

option C: hybrid
- lightweight checks (trigger evaluation) run outside the queue
- only push to queue when action is needed
- avoids waking the full agent loop for no-ops

## relationship to ava-qtcv

ava-qtcv covers the scheduling engine (storage, cron parsing, task definitions). this issue covers how scheduled task execution integrates with the conversation architecture — specifically the queue and agent loop.

## questions

- should heartbeats create visible conversation turns or be silent unless they trigger something?
- how to prevent a long-running scheduled task from blocking user messages?
- should scheduled tasks have their own session or share the active one?
- rate limiting: what if a trigger fires every minute but the agent takes 30s per turn?

## output

- integration design with message queue
- heartbeat message format
- scheduling → queue flow
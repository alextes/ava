---
schema_version: 9
id: ava-4ial
title: write logs to file from startup
priority: P2
status: done
deps: []
tags:
- daemon
- observability
owner: null
created_at: 2026-03-09T21:12:27.302115Z
started_at: 2026-03-09T21:12:30.698986Z
completed_at: 2026-03-11T11:25:30.413256Z
---

write tracing output to a log file in addition to stdout, from the very start (not gated on daemonization).

## motivation

currently logs only go to stdout, which means they're lost on restart and the agent can't audit its own past activity. writing to file from startup enables self-auditing and makes logs persistent across hot-swap restarts.

## implementation

- add tracing-appender crate
- log file path: ~/.ava/ava.log (create dir if not exists)
- use rolling file appender (daily rotation) to prevent unbounded growth
- write to both stdout AND file (layered tracing_subscriber)
- keep existing stdout logging unchanged

## notes

- ava-cqfj (add ava logs subcommand) can build on this once implemented
- file logging should survive hot-swap restarts since exec() preserves the file path
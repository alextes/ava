---
schema_version: 9
id: ava-sm2i
title: add ava stop subcommand
priority: P2
status: done
deps:
- ava-6r4z
tags:
- daemon
owner: null
created_at: 2026-02-09T17:46:59.941496Z
started_at: 2026-03-17T20:21:40.849185Z
completed_at: 2026-03-17T20:24:48.441359Z
---

add `ava stop` subcommand. reads PID from ~/.ava/ava.pid, sends SIGTERM, waits briefly for process to exit, removes PID file. error if PID file doesn't exist or process isn't running.
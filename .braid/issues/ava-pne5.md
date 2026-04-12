---
schema_version: 9
id: ava-pne5
title: daemonize ava start
priority: P2
status: done
deps:
- ava-6r4z
tags:
- daemon
owner: null
created_at: 2026-02-09T17:46:55.975723Z
started_at: 2026-03-16T15:17:39.699641Z
completed_at: 2026-03-16T15:20:45.904082Z
---

change `ava start` to fork to background (traditional unix daemon). after fork: write PID file to ~/.ava/ava.pid, redirect stdout/stderr to ~/.ava/ava.log using tracing_appender, return control to shell. before forking: check if already running via PID file + kill(pid, 0) — if alive, print message and exit.
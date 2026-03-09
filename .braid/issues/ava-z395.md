---
schema_version: 9
id: ava-z395
title: add ava update subcommand
priority: P2
status: done
deps:
- ava-xofd
tags:
- daemon
owner: null
created_at: 2026-02-09T17:47:13.498073Z
completed_at: 2026-03-09T13:54:49.51349Z
---

add `ava update` subcommand. steps: 1) cd to project dir (CARGO_MANIFEST_DIR baked in at compile time), 2) run git pull as child process, 3) run cargo build --release as child process, 4) on success read PID from ~/.ava/ava.pid, 5) send SIGUSR1 to running daemon. if build fails, report error and don't signal. if no daemon running, just build (useful for manual restarts).
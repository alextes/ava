---
schema_version: 9
id: ava-xofd
title: add SIGUSR1 handler for hot-swap exec
priority: P2
status: done
deps:
- ava-pne5
tags:
- daemon
owner: null
created_at: 2026-02-09T17:47:08.175045Z
completed_at: 2026-03-09T13:54:48.985482Z
---

install a SIGUSR1 signal handler in the daemon process. on SIGUSR1, set an AtomicBool flag. in the agent message loop, after each message completes processing, check the flag — if set, exec() into the new binary (same path as current executable) with `start` arg instead of pulling the next message. this provides graceful hot-swap with no partial responses.
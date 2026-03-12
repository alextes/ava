---
schema_version: 9
id: ava-b4fv
title: add self-update LLM tool
priority: P2
status: done
deps:
- ava-z395
tags:
- daemon
- tool
owner: null
created_at: 2026-02-09T17:47:17.490827Z
started_at: 2026-03-12T11:30:02.488145Z
completed_at: 2026-03-12T11:31:23.15828Z
---

expose self-update as an LLM tool the agent can invoke. the tool triggers the same flow as `ava update` CLI — git pull, cargo build, SIGUSR1. no approval needed (per resolved decisions). lets the agent update itself when asked by the user.
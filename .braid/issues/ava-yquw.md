---
schema_version: 9
id: ava-yquw
title: unified context-aware usage log line
priority: P2
status: done
deps:
- ava-oh2z
tags:
- session
- observability
owner: null
created_at: 2026-03-15T22:20:50.254282Z
completed_at: 2026-03-15T22:31:12.657334Z
---

replace the two-branch usage log in agent/mod.rs with a single unified line including context percentage. format: 'context: 42% (84000/200000 tokens), output: 1200, cache: 50000 created / 30000 read'. log at WARN when >60%.
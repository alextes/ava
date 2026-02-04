---
schema_version: 9
id: ava-1q20
title: set up sqlite
priority: P2
status: done
deps: []
tags:
- storage
owner: null
created_at: 2026-02-01T21:29:57.123175Z
started_at: 2026-02-01T22:16:41.528363Z
completed_at: 2026-02-01T22:17:51.789393Z
---

set up sqlite for persistent storage. ava will need to store a lot — sessions, messages, context, etc. use rusqlite or sqlx.
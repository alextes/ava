---
schema_version: 9
id: ava-xi0r
title: implement remember_fact tool
priority: P1
status: done
deps:
- ava-jehm
tags:
- tool
owner: null
created_at: 2026-02-04T21:52:44.908686Z
started_at: 2026-02-05T19:17:19.677756Z
completed_at: 2026-02-05T19:24:57.618847Z
---

tool for agent to store learned facts.

interface:
```
remember_fact(category, key, value)
```

- upserts: insert or update on conflict
- updates updated_at timestamp on update
- source defaults to 'agent'

depends on facts table migration.
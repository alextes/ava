---
schema_version: 9
id: ava-ymov
title: inject facts into system prompt
priority: P1
status: done
deps:
- ava-jehm
tags:
- core
owner: null
created_at: 2026-02-04T21:52:46.763239Z
started_at: 2026-02-05T22:14:05.124212Z
completed_at: 2026-02-05T22:16:27.508419Z
---

load facts from db and inject into system prompt.

format:
```
## known facts

### user
- name: Alex
- timezone: Europe/Amsterdam

### preferences
- response_style: concise
```

constraints:
- max 50 facts
- order by updated_at DESC
- truncate values > 500 chars
- group by category

depends on facts table migration.
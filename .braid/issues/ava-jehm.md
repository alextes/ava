---
schema_version: 9
id: ava-jehm
title: add facts table migration
priority: P1
status: done
deps: []
tags:
- storage
owner: null
created_at: 2026-02-04T21:52:43.214089Z
started_at: 2026-02-04T22:05:09.316518Z
completed_at: 2026-02-04T22:12:05.5011Z
---

add migration for the facts table:

```sql
CREATE TABLE facts (
    id INTEGER PRIMARY KEY,
    category TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'agent',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(category, key)
);

CREATE INDEX idx_facts_category ON facts(category);
CREATE INDEX idx_facts_updated ON facts(updated_at DESC);
```
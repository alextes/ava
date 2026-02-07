---
schema_version: 9
id: ava-v4dl
title: design memory and facts system
priority: P1
status: done
type: design
deps: []
tags:
- core
owner: null
created_at: 2026-02-04T21:37:00.901586Z
started_at: 2026-02-04T21:42:08.879159Z
completed_at: 2026-02-04T21:53:10.887065Z
---

persistent memory for ava to remember facts across sessions.

## aidaemon approach
- SQLite tables: messages (with embeddings) and facts (category, key, value)
- `remember_fact` tool for agent to store knowledge
- facts injected into system prompt under "Known Facts" section
- tri-hybrid retrieval: semantic search, recent context, pinned memories

---

## design decisions

**scope:** global facts, single user (alex), single agent (ava). no per-session isolation.

**no embeddings:** category/key lookup is sufficient for ~100 facts. add embeddings later if retrieval becomes a problem.

**no /remember command:** let agent learn naturally through conversation. skip manual fact entry.

**future: daily reflection:** once we have scheduled tasks, ava reviews conversation logs daily to extract new facts worth noting.

**future: expiry:** facts expire after ~1 week unless:
- fact was useful in recall (injected into prompt and conversation succeeded)
- fact was rediscovered during reflection

---

## schema

```sql
CREATE TABLE facts (
    id INTEGER PRIMARY KEY,
    category TEXT NOT NULL,      -- e.g., "user", "preference", "project"
    key TEXT NOT NULL,           -- unique within category
    value TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'agent',  -- 'agent' or 'user'
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(category, key)
);

CREATE INDEX idx_facts_category ON facts(category);
CREATE INDEX idx_facts_updated ON facts(updated_at DESC);
```

## tool interface

```
remember_fact(category, key, value)
```
- upserts: creates or updates existing fact
- agent uses this to store learned information

```
forget_fact(category, key)
```
- deletes a fact (for cleanup/corrections)
- can skip initially, add when needed

## prompt injection

facts injected into system prompt:

```
## known facts

### user
- name: Alex
- timezone: Europe/Amsterdam

### preferences
- response_style: concise
- code_language: rust
```

**limits:**
- max 50 facts initially (configurable)
- order by updated_at DESC (most recent first)
- truncate value if > 500 chars

---

## implementation issues

after approval, create:
1. add facts table migration
2. implement remember_fact tool
3. inject facts into system prompt
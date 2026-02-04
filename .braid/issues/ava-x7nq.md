---
schema_version: 9
id: ava-x7nq
title: design memory system
priority: P2
status: open
type: design
deps: []
tags:
- tool
owner: null
created_at: 2026-02-01T22:32:41.7411Z
---

design ava's memory/recall system.

look at openclaw's approach for inspiration. start simple:
- sqlite-based memory storage
- key-value or structured memories
- retrieval by recency, relevance

later iterations can add:
- embeddings for semantic search
- vector search (sqlite-vec or similar)
- memory importance/decay

output: schema design and retrieval strategy.
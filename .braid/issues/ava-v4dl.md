---
schema_version: 9
id: ava-v4dl
title: design memory and facts system
priority: P1
status: open
type: design
deps: []
tags:
- core
owner: null
created_at: 2026-02-04T21:37:00.901586Z
---

persistent memory for ava to remember facts across sessions.

## aidaemon approach
- SQLite tables: messages (with embeddings) and facts (category, key, value)
- `remember_fact` tool for agent to store knowledge
- facts injected into system prompt under "Known Facts" section
- tri-hybrid retrieval: semantic search, recent context, pinned memories

## questions to consider
- do we need embeddings for semantic search, or is category/key sufficient for now?
- how many facts before prompt gets too long? pruning strategy?
- should users be able to add/edit facts directly?
- fact expiration or confidence scores?
- separation between "user told me" vs "i learned this"?

## output
- storage schema design
- retrieval strategy
- tool interface for agent
---
schema_version: 9
id: ava-71h9
title: design embeddings for memory retrieval
priority: P3
status: open
type: design
deps: []
tags:
- core
owner: null
created_at: 2026-02-04T21:52:52.755299Z
---

add semantic search via embeddings for memory retrieval.

## prerequisite

for embeddings to make sense, we first need a system that collects many more memories than just facts. possibilities:
- conversation summaries
- full message history with importance scoring
- extracted entities and relationships
- episodic memories (what happened when)

with only ~100 category/key facts, simple lookup is sufficient. embeddings become valuable when we have thousands of memories and need fuzzy/semantic retrieval.

## questions
- what embedding model? local (gte-small) vs api (openai)?
- vector storage: sqlite-vss, qdrant, in-memory?
- when to embed: on write vs batch job?
- retrieval: top-k similar vs hybrid (semantic + recency + importance)?

## references
- aidaemon uses tri-hybrid: semantic search, recent context, pinned memories
- sqlite-vss for embedded vector search
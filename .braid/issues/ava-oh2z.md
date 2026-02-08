---
schema_version: 9
id: ava-oh2z
title: design context compaction for long sessions
priority: P2
status: doing
type: design
deps: []
tags:
- session
- context
owner: alextes
created_at: 2026-02-08T14:08:45.570297Z
started_at: 2026-02-08T16:22:30.371241Z
---

as sessions grow beyond the context window limit, ava needs a strategy for compacting older history while preserving important context.

## problem

with a fixed history window (e.g. last 50 messages), old messages silently disappear from the provider's view. ava loses context about earlier parts of the conversation — names, decisions, topics discussed.

## approaches to research

### summarization
- periodically summarize older messages into a condensed "session summary" block
- inject summary into system prompt alongside facts
- when should summarization trigger? (every N messages? when history exceeds threshold?)
- use the same provider to summarize, or a cheaper/faster model?

### facts extraction
- the `remember_fact` tool already exists — could ava be prompted to proactively extract important facts before they fall off the context window?
- system prompt instruction: "if important information is about to leave your context, use remember_fact to preserve it"

### hierarchical memory
- recent messages: full fidelity (last 50)
- older messages: summarized (last 200, compressed to a paragraph)
- ancient messages: only preserved as facts

### prior art
- claude code uses automatic context compaction when approaching limits
- other AI assistants use similar tiered approaches

## output

architecture recommendation for how ava handles context beyond the sliding window. should complement the session implementation (ava-df0i) and prompt caching strategy (ava-pvpj).
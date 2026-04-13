---
schema_version: 9
id: ava-2o0j
title: design agent-editable base instructions (self-prompt)
priority: P2
status: open
type: design
deps: []
tags:
- memory
- prompt
owner: null
created_at: 2026-04-13T11:16:38.675791Z
---

## problem

every agent harness has a concept of editable base instructions (CLAUDE.md, AGENTS.md, etc.) that shape agent behavior across sessions. ava's memory system (facts, episodes, identity traits) partially fills this role, but has limitations:

1. **no dedicated "instructions" concept** — behavioral rules compete with factual knowledge for the same 50-fact system prompt slots
2. **no always-present guarantee** — as facts accumulate, older ones silently fall out of the system prompt
3. **pull-only retrieval** — the `recall` tool requires the agent to know what to search for; there's no push-based relevance matching

the memory system is already close to being the right solution. identity traits are always included and upsert by key. but they're semantically "who am i" not "how should i behave", and facts (the natural home for behavioral rules) have the recency-limited injection problem.

## proposed approach (tiered)

### tier 1: instructions memory kind (low effort, high value)

add a new memory kind `instructions` to the memories table:

- always included in system prompt (no recency limit, cap at ~2000 chars total)
- agent gets `set_instruction` / `remove_instruction` tools (or reuse remember/forget with kind=instructions)
- renders as `## self-instructions` section in prompt
- upsert semantics by key, like identity traits
- this is the direct analogue to CLAUDE.md but agent-writable

examples of what the agent would store here:
- "always reply in lowercase"
- "prefer exec tool over code blocks"  
- "when user says 'deploy', run the deploy skill first"

### tier 2: relevance-scored fact injection (medium effort)

instead of "most recent 50 facts", score facts by relevance to current message:

- embed user message, cosine-similarity against fact embeddings
- inject top-k most relevant facts instead of most recent
- requires an embedding model (could use a local one or API)
- solves the "old important facts fall out" problem

### tier 3: hierarchical memory with summarization (higher effort)

periodically summarize clusters of related facts/episodes into meta-memories:

- mix of recent granular memories + older summarized ones
- summarization runs as a scheduled task
- like context compaction but for the memory layer

## recommendation

start with tier 1. it covers the most common need (durable behavioral rules that never fall out of context) with minimal code change. the existing memory infra handles storage, the system prompt builder just needs a new section. tier 2 is the natural follow-up for improving factual recall.

## research notes

current system prompt construction: `src/agent/mod.rs:745-843`
prompt formatting: `src/agent/prompt.rs`
memory DB layer: `src/db/memory.rs`
memory limits: 20 identity traits, 50 facts, 20 episodes (all truncated to 500 chars)
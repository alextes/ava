---
schema_version: 9
id: ava-pt3u
title: design core memories always loaded into prompt
priority: P2
status: open
type: design
deps: []
tags:
- core
- memory
owner: null
created_at: 2026-04-07T11:32:25.062624Z
---

introduce a first-class 'core memories' tier for persistent, always-loaded context. this is similar in spirit to a repo-local CLAUDE.md, but for ava's long-term user/assistant relationship.

## motivation

the current memory system has three kinds:
- fact
- episode
- character

and the prompt always injects character traits plus a selected subset of facts/episodes. this works, but it lacks an explicit notion of *always-on, curated, foundational memory*.

some memories should always be present in a fresh session or compacted context window:
- user name
- code directory
- timezone/location
- languages
- work context
- stable assistant character traits
- durable working conventions

these are more like a personal CLAUDE.md than ordinary remembered facts.

## current state

implemented today:
- sqlite-backed memories table with fact / episode / character kinds
- character traits always injected into the prompt
- recent facts and episodes injected based on db retrieval logic
- session compaction and summaries for long conversations

missing today:
- explicit 'always load this' semantics
- distinction between foundational memory and ordinary recallable memory
- a durable, curated layer that survives future prompt shaping changes

## proposal

add a new tier: **core memories**.

properties:
- always loaded into the system prompt (or equivalent always-on prompt section)
- small, curated, stable
- user-editable
- distinct from recent facts/episodes
- survives compaction because it is not derived from session history

possible implementation approaches:

### option 1: extend existing memories table
add a boolean flag like `is_core` to fact and character memories.

pros:
- minimal schema churn
- reuses existing tools and retrieval logic

cons:
- mixes two dimensions (kind + loading policy)
- episodes likely should not be core, so semantics get fuzzy

### option 2: add a new memory kind
add `core` (or `core_fact` / `core_character`) as explicit memory kinds.

pros:
- explicit
- easy to reason about in prompt construction

cons:
- more schema / tool branching
- may duplicate fact vs character structure

### option 3: separate core_memories table
store key/value core memories independently.

pros:
- clean conceptual model
- easy to mirror a CLAUDE.md / profile file

cons:
- duplicates existing memory concepts
- more plumbing

## recommendation

prefer **option 1** initially: add `is_core` to memories, but constrain usage in tooling:
- facts can be core
- character traits can be core
- episodes cannot be core

then prompt construction becomes:
1. base system prompt
2. core character
3. core facts
4. recent / non-core context
5. tasks / skills / dynamic sections

## tooling

potential UX:
- extend `remember` with an optional `core: true` flag for fact/character
- add a `core memories` section to `ava status` or memory inspection tools later
- possibly add a dedicated `remember_core` tool if clearer

## open questions

- should core memories be ordered manually?
- should there be a size cap (e.g. max 20 entries / max chars)?
- should the user have a file-backed editing path (e.g. `~/.ava/CORE.md`) that syncs into db?
- should all current character traits implicitly be core, or remain as-is?
- should core memories appear separately from ordinary facts in recall output?

## output

design and implementation plan for a core memory tier that is always injected into the prompt, analogous to a personal CLAUDE.md.

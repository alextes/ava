---
schema_version: 9
id: ava-g3o2
title: design task scratchpad with system prompt injection
priority: P2
status: open
type: design
deps: []
tags:
- tool
- core
owner: null
created_at: 2026-02-09T16:29:01.324179Z
---

a persistent task scratchpad the agent can write to during conversations without derailing the current thread. tasks are stored in SQLite, managed via a dedicated tool, and surfaced in the system prompt so the agent always sees pending work.

## motivation

the agent often notices things mid-conversation that it should handle later — CI failures, remaining steps in a multi-step plan, follow-ups mentioned by the user. currently it has no structured way to capture these. a markdown file (like openclaw's HEARTBEAT.md) is primitive and unreliable. a proper SQLite-backed task table with a tool gives the agent reliable tracking.

## what exists

- SQLite database with migrations (schema v6)
- memory system with system prompt injection (facts, episodes, character traits)
- tool system with clear patterns for adding new tools
- message queue for sequential processing
- cron tool in design (ava-vg4w) — not yet implemented

## design: hybrid task scratchpad (option D)

### phase 1: table + tool + system prompt injection (no cron dependency)

**SQLite table** (`tasks`):

| column | type | notes |
|--------|------|-------|
| id | INTEGER PRIMARY KEY | auto-increment |
| title | TEXT NOT NULL | short summary, shown in system prompt |
| detail | TEXT | full description, shown when agent queries |
| status | TEXT | 'pending' or 'done' |
| created_at | TEXT | datetime default now |
| completed_at | TEXT | nullable, set when done |

title vs detail split is key: the system prompt only shows titles to keep token cost bounded. if the agent needs full context it calls the tool with action=get.

**tool** (`tasks`, 4 actions):

- `add`: create a task with title + optional detail. returns task id.
- `list`: show all pending tasks (title + id). optionally include done tasks.
- `get`: show full detail for a specific task by id.
- `done`: mark a task complete by id.

no approval required — these are the agent's own notes.

**system prompt injection**: append pending task titles to the system prompt, same pattern as memories. format:

```
## pending tasks
1. [id:3] investigate CI failure on main
2. [id:7] fix test_session_persistence
3. [id:12] review PR #42 tomorrow
```

just titles and ids. no detail. this keeps the injection bounded even with 20+ tasks. worst case ~20 lines of short text.

if there are zero pending tasks, omit the section entirely (no tokens burned).

### phase 2: synthetic messages via cron (future, after ava-vg4w)

when the cron system exists, add a recurring entry that checks for pending tasks and pushes a synthetic message into the queue: "you have N pending tasks. review your task list and make progress where possible."

this gives the agent the "wake up and work on your backlog" behavior without a separate heartbeat mechanism.

### edge cases

- 20+ pending tasks with large details: system prompt only shows titles, so bounded. detail is on-demand via get action.
- stale tasks that will never be done: could add a `stale after N days` cleanup later, not needed for v1.
- duplicate tasks: no uniqueness constraint on title — agent can add duplicates. fine for v1.
- task ordering: listed by created_at ascending (oldest first). no priority field for v1.

## files to change

| file | change |
|------|--------|
| src/db/mod.rs | add migration v7 with tasks table, add/list/get/done query methods |
| src/db/migrations.rs | migration SQL for tasks table |
| src/tool/mod.rs | add tasks tool definition and handler |
| src/agent/mod.rs | inject pending task titles into system prompt |

## output

one implementation issue covering the full phase 1.
---
schema_version: 9
id: ava-qtcv
title: design scheduled tasks
priority: P2
status: done
type: design
deps:
- ava-lzbb
tags:
- core
owner: null
created_at: 2026-02-04T21:37:12.985608Z
started_at: 2026-02-09T21:22:18.686492Z
completed_at: 2026-02-10T07:52:48.706648Z
---

recurring and one-time automated task execution.

**status: fully implemented** — the cron tool (ava-vg4w) shipped this. documenting the design as-built below.

## resolved decisions

### natural language parsing → not used, rely on LLM

no parsing library needed. the LLM translates user intent into ISO 8601 datetimes (one-time) or 5-field cron expressions (recurring). this is simpler and more flexible than any NLP library.

### agent self-scheduling → yes, no approval required

the cron tool is available to the agent without approval gating. the agent can create, list, and cancel schedules freely. this enables routines, follow-ups, and reminders.

### task output → queued as agent message via telegram

when a schedule fires, its `prompt` is sent as a `QueuedMessage` into the agent's message queue. the agent processes it like any user message, and the response goes back via telegram. this is simple and reuses the existing message pipeline.

### failure handling → log and continue, no retry

errors during schedule firing (queue send failure, db errors) are logged but not retried. recurring schedules keep their cadence — if one firing fails, the next one still runs on time. no dead-letter queue or failure tracking. acceptable for now.

### timezone handling → UTC only

all times stored and compared in UTC. cron expressions fire in UTC. users must account for this when scheduling. local timezone support deferred — the LLM can help users convert.

### resource limits → none, trusted context

schedules run in the same agent context as user messages. no sandboxing or resource limits. the prompt is trusted since only the agent (or user via agent) can create schedules.

## as-built architecture

### storage schema (`schedules` table)

```sql
CREATE TABLE schedules (
    id INTEGER PRIMARY KEY,
    description TEXT NOT NULL,
    prompt TEXT NOT NULL,
    cron_expr TEXT,           -- NULL for one-time, cron expression for recurring
    next_run_at TEXT NOT NULL, -- YYYY-MM-DD HH:MM:SS
    last_run_at TEXT,
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_schedules_active_next ON schedules(next_run_at) WHERE active = 1;
```

### scheduling engine (`src/scheduler.rs`)

- background tokio task, 60-second check interval
- queries `due_schedules()` for `next_run_at <= now AND active = 1`
- fires each due schedule by sending its prompt to the message queue
- advances recurring schedules to next cron occurrence via `croner` crate
- deactivates one-time schedules after firing

### cron tool (`src/tool/cron.rs`)

actions: `schedule`, `list`, `cancel`. input uses ISO 8601 for one-time (`run_at`) and 5-field cron for recurring (`cron_expr`). validates cron syntax with `croner` v3.
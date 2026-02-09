---
schema_version: 9
id: ava-cior
title: research heartbeat patterns in AI agents
priority: P2
status: doing
type: design
deps: []
tags:
- core
owner: agent-one
created_at: 2026-02-09T08:37:56.600644Z
started_at: 2026-02-09T08:37:58.865215Z
---

explore how openclaw, aidaemon, nanobot, and other AI agent projects implement heartbeat/cron/scheduled wake-up patterns. how do they trigger periodic runs, what context does the agent get, how do they prevent runaway costs, and how do they persist state between heartbeats. output: summary of patterns and recommendations for ava heartbeat design.

## findings

### openclaw

the most mature heartbeat system of the projects researched.

**trigger mechanism**: internal gateway timer with configurable interval (default 30 minutes). the gateway checks whether the agent is within "active hours" before firing. heartbeats are skipped outside active hours entirely.

**context on wake**: the agent receives a `HEARTBEAT.md` file — a structured task checklist with prioritized items. this acts as a persistent TODO list between heartbeats. the agent also gets `HEARTBEAT_OK` — a no-op signal meaning "nothing to do, go back to sleep" which prevents the agent from hallucinating work.

**cost control**:
- cheap model routing: heartbeat-triggered runs use a cheaper model for initial triage, only escalating to expensive models when real work is detected
- active hours: heartbeats simply don't fire outside configured windows
- lane queue concurrency: limits how many tasks can run simultaneously
- BODHI cost guardrails: budget-aware spending limits per time period

**state persistence**: `HEARTBEAT.md` file persisted to disk. the agent reads it on wake, updates it during the run, and writes it back before sleeping.

### aidaemon

a rust daemon with the most sophisticated scheduling of the projects researched.

**trigger mechanism**: internal `SchedulerManager` with a 30-second tick loop. supports both cron expressions and natural language scheduling (e.g. "every morning at 9am"). scheduled tasks are persisted to SQLite as `scheduled_tasks` rows. also has an `Event` broadcast channel for reactive triggers (not just time-based).

**context on wake**: scheduled tasks carry a stored prompt/description. the agent loads conversation history from SQLite and the task description. the scheduler fires events that the main loop picks up and routes to the agent.

**cost control** (5-layer system):
1. stall detection: detects when the agent is looping without progress
2. iteration limits: hard cap on tool-call iterations per run
3. daily token budget: aggregate token spend limit per 24h window
4. watchdog timer: kills runs that exceed wall-clock time limits
5. sub-agent limits: nested agent spawns have their own budget caps

**state persistence**: SQLite for everything — scheduled tasks, conversation history, token usage counters. the daemon process is long-lived, so in-memory state persists between ticks.

### nanobot

a python project, simpler approach.

**trigger mechanism**: python `asyncio` timers. configurable interval. follows the openclaw convention of `HEARTBEAT.md` + `HEARTBEAT_OK`.

**context on wake**: reads `HEARTBEAT.md` for task list. has a pre-LLM short-circuit: if the task board is empty, returns `HEARTBEAT_OK` immediately without making an API call. this is a clever cost optimization.

**cost control**: minimal — max 20 iterations per heartbeat run. no budget tracking or stall detection.

**state persistence**: JSONL session files. simple append-only logs.

### other frameworks

**letta (formerly memgpt)**: interesting "agent-requested" heartbeat — the agent itself decides to call a `heartbeat` tool to request continuation. also has "sleep-time agents" that process memories in the background during idle periods. not cron-based; more of a continuation mechanism.

**langgraph**: supports cron-based triggers with both stateless runs (fresh context each time) and stateful threads (resume from checkpoint). built on the langgraph platform, not self-hosted.

**temporal ambient agents**: durable workflow execution. the agent runs as a temporal workflow, which handles retries, timeouts, and crash recovery automatically. very infrastructure-heavy.

**autogpt**: has a "continuous mode" that loops without user approval. no real scheduling — just a tight loop. no cost controls beyond iteration limits.

**crewai / autogen**: no native scheduling or heartbeat support. focused on multi-agent orchestration within a single run.

## patterns summary

| aspect | openclaw | aidaemon | nanobot |
|--------|----------|----------|---------|
| trigger | internal timer (30m) | scheduler (30s tick + cron) | asyncio timer |
| context | HEARTBEAT.md checklist | SQLite task + history | HEARTBEAT.md |
| no-op signal | HEARTBEAT_OK | n/a | HEARTBEAT_OK |
| cheap triage | yes (model routing) | no | yes (pre-LLM check) |
| cost control | multi-layer (budget, hours, routing) | 5-layer (stall, iter, budget, watchdog, sub-agent) | minimal (20 iter cap) |
| persistence | file-based | SQLite | JSONL |
| language | python | rust | python |

## key takeaways for ava

1. **HEARTBEAT.md pattern is proven**: both openclaw and nanobot use it. a structured task checklist gives the agent clear context without replaying full conversation history. ava could store this in the database rather than a file.

2. **pre-LLM short-circuit saves cost**: nanobot's approach of checking the task board before making any API call is simple and effective. if there's nothing to do, don't call the model at all.

3. **active hours matter**: openclaw's active hours window prevents pointless 3am heartbeats. configurable schedule (e.g. "weekdays 9-18") is a must.

4. **cost control is critical**: aidaemon's 5-layer approach is the gold standard but may be over-engineered for ava's initial version. minimum viable: iteration limit per heartbeat + daily token budget.

5. **cron expressions are the right abstraction**: aidaemon shows that cron expressions are flexible enough for most scheduling needs while being well-understood. natural language scheduling is a nice-to-have on top.

6. **the no-op signal prevents hallucinated work**: without an explicit "nothing to do" convention, agents tend to invent tasks. HEARTBEAT_OK or equivalent is important.

## recommendations for ava (ava-hmpj)

**minimal viable heartbeat**:
- configurable interval timer (default 30 minutes, stored in db)
- active hours window (skip heartbeat outside configured hours)
- heartbeat task list stored in database (equivalent to HEARTBEAT.md)
- pre-LLM short-circuit: if task list is empty, skip API call
- iteration limit per heartbeat run (e.g. 10 tool calls max)
- HEARTBEAT_OK convention in system prompt

**follow-up enhancements**:
- cron expression support (instead of fixed interval)
- daily token budget tracking
- cheap model routing for triage (check tasks → escalate if needed)
- agent-proposed heartbeat tasks (via a tool, similar to manage_rules)
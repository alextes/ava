---
schema_version: 9
id: ava-hmpj
title: design cron and heartbeat integration with message queue
priority: P2
status: done
type: design
deps:
- ava-lzbb
tags:
- core
owner: null
created_at: 2026-02-08T16:00:51.229416Z
started_at: 2026-02-21T15:24:14.403533Z
completed_at: 2026-02-21T15:28:47.154921Z
---

design how cron jobs and periodic heartbeats integrate with the message queue architecture (ava-rv0i).

## status: already implemented (option C hybrid)

the scheduler (ava-qtcv / `src/scheduler.rs`) already implements a hybrid approach. this design doc captures the as-built architecture, evaluates the open questions, and recommends minor improvements.

## as-built architecture

### the current pattern: hybrid (option C)

the scheduler runs as a background tokio task with a 60-second poll interval. it performs lightweight checks outside the queue and only pushes messages when action is needed:

1. **cron schedules**: queries `db.due_schedules()` each tick. only sends a `QueuedMessage` when a schedule is actually due. advances the schedule (next occurrence or deactivate) immediately after queuing.
2. **task board nudge**: every 30 minutes (configurable via `AVA_TASK_CHECK_INTERVAL_SECS`), checks `db.pending_task_titles()`. only sends a nudge message if there are pending tasks.

both inject `QueuedMessage`s into the same `mpsc` queue that user messages use. the agent loop processes them identically — there's no special-casing for scheduled vs user messages.

### why hybrid works well

- **no wasted agent turns**: the scheduler evaluates triggers cheaply (a single SQL query) and only wakes the full agent loop when there's real work.
- **unified processing**: once a scheduled message enters the queue, it's processed exactly like a user message — same session, same tools, same conversation history. no parallel execution paths to reason about.
- **natural priority**: user messages and scheduled messages share one FIFO queue. if the user is actively chatting, scheduled messages queue behind the current turn and get processed next. no starvation because agent turns are bounded (40 tool rounds max).

## resolved design questions

### should heartbeats create visible conversation turns?

**yes, and they already do.** when a cron schedule fires, its prompt becomes a user-role message in the conversation. the agent processes it and responds via telegram. this is the right behavior — scheduled tasks are meant to produce output (reminders, check-ins, reports). if a scheduled task has nothing to report, the agent can say so briefly or use the `complete` tool for silent completion.

the task board nudge also creates a visible turn. this is intentional — it prompts the agent to review and act on pending tasks, and the user sees the agent's response.

### how to prevent long-running scheduled tasks from blocking user messages?

**the existing tool budget handles this.** each turn has a 40-round tool budget. if a scheduled task triggers an expensive multi-tool workflow, it's bounded to 40 rounds like any other turn. user messages queue behind it and get processed next.

in practice, most scheduled prompts are simple ("check the weather", "review pending tasks") and complete in 1-3 tool rounds. the 60-second scheduler interval also provides natural spacing — even if a scheduled task takes 30 seconds, there's a ~30 second gap before the next check.

**if this becomes a real problem** (measurable user-perceived latency from scheduled tasks), the fix would be priority queuing — user messages skip ahead of scheduled ones. but this adds complexity and isn't needed today.

### should scheduled tasks have their own session or share the active one?

**share the active session (current behavior).** scheduled tasks run in the same conversation context as user messages. this is correct because:

- the agent needs conversation history to handle scheduled tasks well (e.g. a reminder about something discussed earlier)
- session isolation would require a separate agent instance or session-switching logic
- the single-writer model (one agent loop, one session) is simpler and avoids concurrency issues

### rate limiting: what if a trigger fires every minute but the agent takes 30s per turn?

**the queue absorbs bursts.** the mpsc channel has a buffer of 64 messages. if scheduled messages queue up faster than the agent processes them, they buffer. this handles short bursts fine.

**for sustained overload** (a cron that fires every minute + 30s agent turns = 100% utilization with no room for user messages), the answer is: don't create cron schedules that fire faster than the agent can process them. the cron tool's minimum granularity is 1 minute. agent turns for simple scheduled prompts typically take 5-15 seconds. a schedule firing every minute is sustainable.

if we needed enforcement, the scheduler could skip firing a schedule if the queue depth exceeds a threshold. but this isn't a problem in practice for a single-user assistant.

## current limitations and future improvements

### channel-agnostic scheduling

the scheduler currently hardcodes `ChannelKind::Telegram` and `ResponseSink::Telegram` for all scheduled messages. this means scheduled task responses always go to telegram, even if the user primarily uses a different channel.

**recommended fix**: add a `ChannelKind::System` variant for scheduled messages. the agent loop would route system-originated responses to a configurable default channel, or to the most recently active channel.

### heartbeat as a distinct concept

the issue mentions "heartbeats" as a separate concept from cron schedules. currently there's no dedicated heartbeat — just the task board nudge. a heartbeat would be a periodic "check in on the user" message.

**recommendation**: heartbeats don't need new infrastructure. they're just cron schedules with a proactive prompt. the agent can create a heartbeat schedule via the cron tool (e.g. `0 9 * * *` with prompt "good morning check-in"). no new code needed — the existing scheduler handles it.

### silent scheduled checks

some scheduled tasks might want to check a condition and only message the user if something is notable (e.g. "check if any emails need attention" — only respond if there are important emails). currently every scheduled message creates a visible conversation turn.

**recommendation**: this is already possible. the agent can use the `complete` tool to end a turn silently if the check finds nothing noteworthy. the prompt can instruct: "check X. if nothing notable, use the complete tool without responding." no new infrastructure needed.

## summary

the cron/heartbeat integration with the message queue is already implemented and working well. the hybrid approach (option C) is the right choice — lightweight polling outside the queue, synthetic messages pushed only when action is needed. the sequential agent loop naturally serializes scheduled and user messages without race conditions.

**no implementation issues needed** — the architecture is sound. the only minor improvement worth tracking separately is making scheduled messages channel-agnostic (removing the telegram hardcoding).
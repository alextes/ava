---
schema_version: 9
id: ava-0i2i
title: add Progress enum and channel to agent.process()
priority: P2
status: open
deps:
- ava-1v4u
owner: null
created_at: 2026-04-13T10:21:02.40805Z
---

add Progress enum (Thinking, ToolRound { round, total }, Compacting) to message.rs. add mpsc::Sender<Progress> parameter to agent.process(). emit events at key points: before provider.complete() (Thinking on first round), after tool_rounds increment (ToolRound), before compaction (Compacting). CLI/test callers pass a sender whose receiver is dropped.
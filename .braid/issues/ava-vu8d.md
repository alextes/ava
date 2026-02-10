---
schema_version: 9
id: ava-vu8d
title: parallelize independent tool call execution
priority: P2
status: done
deps: []
tags:
- core
- agent
owner: null
created_at: 2026-02-10T08:11:42.519564Z
started_at: 2026-02-10T08:11:52.121348Z
completed_at: 2026-02-10T08:16:12.127382Z
---

the agent loop executes tool calls sequentially in a for loop. when the LLM returns multiple independent tool calls (e.g. reading 10 files), they should execute concurrently using tokio::JoinSet or futures::join_all. edge cases: the complete tool can exit early, and tools can request provider switches — handle these after the parallel batch completes.
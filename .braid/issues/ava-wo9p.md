---
schema_version: 9
id: ava-wo9p
title: add ContextUsage struct and computation in agent loop
priority: P2
status: done
deps:
- ava-oh2z
tags:
- session
- observability
owner: null
created_at: 2026-03-15T22:20:48.482577Z
started_at: 2026-03-15T22:22:58.215396Z
completed_at: 2026-03-15T22:25:20.054661Z
---

add ContextUsage struct (input_tokens, output_tokens, context_window, usage_percent, compacted, compaction_count). compute after each provider call from response.usage + context_window(). track compaction_count in the agent loop.
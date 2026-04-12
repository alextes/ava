---
schema_version: 9
id: ava-t0rt
title: context usage percentage ignores cached tokens
priority: P2
status: done
deps: []
tags:
- observability
- agent
owner: null
created_at: 2026-03-17T21:03:29.904426Z
started_at: 2026-03-17T21:03:32.705227Z
completed_at: 2026-03-17T21:03:35.786587Z
---

context usage percentage was calculated using only input_tokens, ignoring cache_read_tokens. with a 1M token window and 135k cached tokens, usage showed as 0% instead of ~14%.

## fix

include cache_read_tokens in the total: total = input_tokens + cache_read_tokens. fixed in agent/context.rs ContextUsage::compute().
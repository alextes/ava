---
schema_version: 9
id: ava-s64v
title: add cache-aware cold-resume prompt for cold cache scenarios
priority: P2
status: done
deps: []
tags:
- cache
- session
- cost
- telegram
owner: null
created_at: 2026-04-21T06:06:51.351982Z
started_at: 2026-04-21T06:06:54.649808Z
completed_at: 2026-04-26T12:40:23.303061Z
acceptance:
- track last_completion_at on per-chat session state
- Provider trait exposes cache_ttl() (anthropic=5min, openai=24h, openrouter=0)
- new src/pricing.rs with per-model dollar-per-MTok table and lookup
- before first complete() on fresh user turn, if elapsed > cache_ttl and last_input_tokens >= 10000, prompt user
- 'telegram inline keyboard: keep/clear, 5min timeout defaults to keep'
- clear drains chat buffer and starts fresh session
- skip prompt in autonomous mode (no telegram chat attached)
- prompt message shows token count, cache-age, and estimated reload cost in USD
---

detect when the prompt cache has gone cold on a new user turn, estimate replay cost, and ask user how to proceed (keep/clear). design plan at ~/.claude/plans/iterative-orbiting-bird.md

note: a summarize button was originally proposed but dropped — on cold cache the summarize call costs the same as keep (you pay full read price either way), only saving on subsequent turns. cheap-model summarize is tracked separately under ava-smqa; once that lands, a summarize button can be re-added here.
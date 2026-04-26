---
schema_version: 9
id: ava-smqa
title: summarize/compact via cheap model to avoid paying active-model rate
priority: P2
status: open
deps: []
tags:
- cache
- cost
- compaction
owner: null
created_at: 2026-04-26T08:30:23.808761Z
acceptance:
- compaction (and cold-resume summarize, if added) routes through a configurable cheap model (e.g. haiku-4-5)
- summary is then re-injected into active-model session, replacing summarized messages
- Provider trait or AnyProvider helper exposes which provider/model to use for compaction
- pricing.rs validates the cheap model is actually cheaper than the active one before routing
---

context: when compaction (or a cold-resume summarize) runs on the active model, you pay full read price for the same tokens you would have paid as 'keep'. only future turns benefit from the smaller prefix. on cold cache this is barely a win.

if instead summarization runs on a cheaper model (e.g. claude-haiku-4-5 at $1/MTok vs opus-4-7 at $5/MTok), the read cost drops ~80% and the active model resumes on the compressed context.

worked example, 50k cold conversation:
- keep on opus-4-7: ~$0.25 (full context preserved)
- summarize on opus-4-7: ~$0.275 first turn (worse than keep unless many follow-ups)
- summarize on haiku-4-5: ~$0.05 read + ~$0.025 next-turn read = ~$0.075 (70% cheaper than keep)

scope:
- modify src/agent/compaction.rs to optionally use a different provider for the summarization call
- expose configuration (env var or settings)
- once landed, the cold-resume prompt (ava-s64v) can re-introduce a [summarize] button that does this
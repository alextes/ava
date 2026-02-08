---
schema_version: 9
id: ava-dq8g
title: add context usage observability (token counting / logging)
priority: P2
status: done
deps: []
tags:
- session
- observability
owner: null
created_at: 2026-02-08T14:54:53.226587Z
started_at: 2026-02-08T15:43:53.582169Z
completed_at: 2026-02-08T15:47:40.266729Z
---

add visibility into how much context is being used per API call.

## motivation

with the growing window session model, context grows over time. we need to know when we're approaching limits — both for operational awareness and to inform when compaction should trigger.

## scope

- log the approximate token count of the message history on each provider call
- expose session stats (message count, estimated tokens) via CLI subcommand
- consider showing context usage in telegram responses (optional, maybe as a /status command)
- track provider response usage fields (anthropic returns input/output token counts)

## output

implementation plan or direct implementation — simple enough to not need a full design.
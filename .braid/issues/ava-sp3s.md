---
schema_version: 9
id: ava-sp3s
title: design reasoning token legibility in history CLI
priority: P2
status: open
type: design
deps:
- ava-wo9p
- ava-yquw
- ava-fmgy
- ava-vkoh
- ava-prx4
tags:
- observability
- session
owner: null
created_at: 2026-02-11T11:20:21.36972Z
---

design how to surface reasoning/thinking tokens in the `ava history` CLI command.

## context

we now log reasoning tokens from the OpenAI Responses API (`usage.output_tokens_details.reasoning_tokens`) in the provider usage tracing. this tells us how many of the output tokens were spent on internal reasoning (as opposed to visible response text). anthropic's extended thinking works differently — thinking content arrives as `thinking` content blocks, not as a separate usage field — but the concept is analogous.

reasoning tokens are interesting because:

- they're billed as output tokens but produce no visible text
- they can be a significant portion of the response (hundreds to tens of thousands of tokens)
- understanding them helps debug cost and latency

## scope

- how should `ava history --full` display reasoning token counts per turn?
- should reasoning token info be persisted to the DB (in session messages or a separate table)?
- should we surface anthropic thinking content alongside openai reasoning tokens, or treat them separately?
- consider a `--reasoning` or `--verbose` flag on the history command to toggle this detail

## non-goals

- streaming reasoning content to chat channels (CLI/telegram) — this is more of a heavyweight debugging tool, not suited for small chat UIs
- enabling extended thinking on the anthropic provider (separate concern)

## output

design doc specifying what to show, where to store it, and how it integrates with the existing history CLI.
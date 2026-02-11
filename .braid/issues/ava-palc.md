---
schema_version: 9
id: ava-palc
title: design context usage legibility
priority: P2
status: open
type: design
deps:
- ava-oh2z
tags:
- session
- observability
owner: null
created_at: 2026-02-08T16:17:47.059116Z
---

make context usage visible and actionable — show how much of the model's context window is consumed and what headroom remains.

## motivation

we now log raw token counts per provider call (ava-dq8g), but this doesn't tell us *how full* the context is relative to the model's limit. this matters because:

- different models have different context limits (200k for claude, 128k for gpt-5)
- compaction needs a trigger threshold ("compact when context > 80%")
- humans need visibility when compaction isn't perfect and manual intervention is needed (e.g. starting a new session)

## depends on

ava-oh2z (context compaction design) — the legibility layer should reflect whatever compaction strategy is chosen. if compaction uses summarization, the display should show "N tokens used (M summarized)". if it uses fact extraction, show how many facts were auto-extracted.

## scope

- model-aware context capacity (map model ID to max tokens)
- percentage-based context usage display (in logs, status, and optionally telegram /status)
- clear warning when approaching limits (e.g. >80% capacity)
- show compaction state (how much was compacted, when, how)

## notes

### cached tokens from OpenAI Responses API

as of the migration to OpenAI's Responses API (replacing Chat Completions), cached token counts are available in the response at `usage.input_tokens_details.cached_tokens`. this is analogous to anthropic's `cache_read_tokens`. currently we don't extract this, but it would let us show cache hit rates for OpenAI alongside anthropic — useful for understanding actual cost and whether the conversation prefix is being reused effectively.

## output

design doc specifying what to show, where, and how it integrates with the compaction system.
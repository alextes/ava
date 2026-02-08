---
schema_version: 9
id: ava-pvpj
title: design prompt caching strategy for session history
priority: P2
status: open
type: design
deps: []
tags:
- session
- cost
owner: null
created_at: 2026-02-08T14:08:33.778201Z
---

when ava sends conversation history to the provider, we need to understand how prompt caching interacts with our sliding context window approach.

## core question

if we send the last 50 messages as context, and the conversation advances (message 1 falls off, message 51 is added), does the provider's prompt cache invalidate entirely? or can it partially match the 49 overlapping messages?

## research needed

### anthropic prompt caching
- how does anthropic's prompt caching work? is it prefix-based (must match from the start)?
- if the system prompt stays the same but the first message in the history changes, is the cache wiped?
- what are the pricing differences between cached vs uncached input tokens?
- does anthropic offer any session/stateful API that would let us avoid re-sending history?
- what's the minimum cacheable prefix length?

### openai prompt caching
- how does openai handle caching for conversation history?
- same prefix-matching question — does shifting the window invalidate everything?
- any session-like APIs that avoid re-sending?

### strategies to explore
- **fixed prefix approach**: always start history from message 1 (growing window until compaction) so the prefix is stable and cacheable
- **provider-managed sessions**: let the provider store the conversation, we just send new messages (anthropic stateful API? openai threads?)
- **hybrid**: provider-managed when available, full-history fallback when switching providers
- **system prompt as cache anchor**: keep system prompt + facts stable, accept that history portion may not cache well

## output

architecture recommendation on how to structure API calls to maximize cache hits and minimize input token costs. should inform the session implementation (ava-df0i).
---
schema_version: 9
id: ava-pvpj
title: design prompt caching strategy for session history
priority: P2
status: done
type: design
deps: []
tags:
- session
- cost
owner: null
created_at: 2026-02-08T14:08:33.778201Z
started_at: 2026-02-08T14:17:02.223515Z
completed_at: 2026-02-08T14:41:19.306014Z
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

## research findings

### both providers are prefix-based

both anthropic and openai use prefix-based caching. the cache key is computed from the start of the prompt forward. if the first message changes, the entire cache invalidates.

| scenario | anthropic | openai |
|----------|-----------|--------|
| sliding window (drop msg 1, add msg 51) | 0% hit — new prefix | 0% hit — new prefix |
| growing window (keep all, add msg 51) | full hit — prefix stable | full hit — prefix stable |

**this is the critical finding:** a sliding window (e.g. "last 50 messages") destroys caching entirely. a growing window (always start from message 1) preserves it.

### anthropic specifics

- **opt-in** via `cache_control` breakpoints on message content blocks (up to 4 per request)
- **pricing**: cache writes cost 1.25x base input price. cache reads cost **0.1x** (90% savings)
- **TTL**: 5 minutes (refreshed on each use), or 1 hour at 2x write cost
- **minimum**: 1,024 tokens (sonnet), 4,096 tokens (opus, haiku)
- **no stateful API**: Messages API is fully stateless. must always send full history
- **automatic lookback**: system checks up to 20 blocks backward from a breakpoint to find cached prefixes
- no beta header required anymore — `cache_control` parameter is GA

### openai specifics

- **automatic** — no opt-in needed, works for prompts over 1,024 tokens
- **pricing**: cached reads cost **50%** of base input price
- **TTL**: 5-10 minutes, max 1 hour (24h with extended retention option)
- **minimum**: 1,024 tokens, 128-token increments
- **Threads/Assistants API**: stateful alternative where openai stores the conversation. but it's a separate API with known reliability issues, and locks you into openai
- optional `prompt_cache_key` parameter for better routing

### provider-managed sessions: not recommended

- anthropic has no stateful API at all
- openai Threads API has reliability concerns and locks you into openai
- both conflict with ava's cross-provider design (switch_model tool)
- sending full history is the right approach — it's provider-agnostic and works with caching

## architecture recommendation

### use a growing window, not a sliding window

instead of "last 50 messages," send ALL messages from the start of the session. this keeps the prefix stable and maximizes cache hits.

the session lifecycle becomes:
1. **growing phase**: every turn appends to the history. the entire history is sent to the provider. the prefix caches perfectly — each turn only pays full price for the new exchange
2. **compaction trigger**: when history exceeds a token budget (e.g. ~80% of context window), trigger compaction
3. **compaction**: summarize older messages into a "session summary" block. this summary becomes the new stable prefix. messages after compaction grow from there
4. **repeat**: the growing/compaction cycle continues indefinitely

```
turn 1:  [system] [user A]                          → all new (cache write)
turn 2:  [system] [user A] [asst A] [user B]        → prefix cached, only [user B] new
turn 3:  [system] [user A] [asst A] [user B] [asst B] [user C]  → prefix cached
...
turn N:  context too large → compact older messages into summary
turn N+1: [system] [summary] [recent msgs] [user X] → new prefix (cache write), then grows again
```

### anthropic: add cache_control breakpoints

the anthropic provider should add `cache_control: {"type": "ephemeral"}` to the last message in the conversation history (before the new user message). this tells anthropic to cache everything up to that point.

placement strategy:
- breakpoint 1: end of system prompt (stable, caches tools + system instructions)
- breakpoint 2: end of the last assistant message in history (caches conversation history)

this way, on each turn:
- system prompt: cached (0.1x cost)
- conversation history up to last turn: cached (0.1x cost)
- new user message + response: full price

### openai: nothing to do

caching is automatic. as long as the prefix is stable (growing window), it just works.

### impact on session design (ava-df0i)

the session design (ava-er4q) proposed `MAX_HISTORY_MESSAGES = 50` as a sliding window. this should change to:

1. **no message count limit** — send all messages (growing window)
2. **add a token budget** — estimate tokens in history, trigger compaction when approaching context limit
3. **compaction is a separate concern** — the ava-oh2z issue covers this. for MVP, we can start with a generous growing window and defer compaction. context limits won't be hit for casual conversations (50 exchanges ≈ 25k tokens, well within 200k context windows)
4. **anthropic provider needs cache_control support** — add breakpoints to the API request

### implementation changes needed

| file | change |
|------|--------|
| `src/provider/anthropic.rs` | add `cache_control` breakpoints to system prompt and last history message |
| `src/agent/mod.rs` | use growing window (load all messages, not last N). defer compaction to ava-oh2z |
| `src/db/mod.rs` | `load_messages()` loads all messages for session (no limit param, or very high default) |

### cost comparison

for a 50-message conversation (≈25k history tokens) on sonnet:

| strategy | anthropic cost per turn | openai cost per turn |
|----------|------------------------|---------------------|
| sliding window (no cache) | 25k × $3/MTok = $0.075 | 25k × $2.50/MTok = $0.063 |
| growing window + cache | 25k × $0.30/MTok = $0.0075 | 25k × $1.25/MTok = $0.031 |
| **savings** | **90%** | **50%** |

over hundreds of turns this is significant.

## output

architecture recommendation: **growing window + anthropic cache breakpoints**. this informs ava-df0i (session implementation) — change from sliding window to growing window, and add cache_control support to the anthropic provider. compaction deferred to ava-oh2z.
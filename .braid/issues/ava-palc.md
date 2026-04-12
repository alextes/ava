---
schema_version: 9
id: ava-palc
title: design context usage legibility
priority: P2
status: done
type: design
deps:
- ava-oh2z
tags:
- session
- observability
owner: null
created_at: 2026-02-08T16:17:47.059116Z
completed_at: 2026-03-15T22:22:42.986762Z
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

## design (2026-02-10, ava agent)

### ContextUsage struct

a lightweight value type computed after each provider call in the agent loop:

```rust
pub struct ContextUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub context_window: u32,
    pub usage_percent: f64,       // input_tokens / context_window * 100
    pub compacted: bool,          // whether compaction has happened this session
    pub compaction_count: u32,    // how many times compaction has run
}
```

computed from the existing `Usage` struct + `context_window()` on the active provider.
no new API calls needed — everything is already available in the agent loop.

### where it shows up

**1. structured log line (every provider call)**

replace the current two-branch usage log in `agent/mod.rs` (lines 100-116) with a
single unified line that includes context percentage:

```
INFO context: 42% (84000/200000 tokens), output: 1200, cache: 50000 created / 30000 read
```

one line, always the same shape, easy to grep. log at WARN level when above 60%.

**2. `ava status` CLI**

extend the existing status command (main.rs lines 71-80) to show context usage.
requires persisting the latest usage snapshot in the session row.

```
ava 0.5.0
db: /Users/alex/.local/share/ava/ava.db
session: 1 (47 messages)
context: 42% (84000/200000 tokens)
model: anthropic/claude-sonnet-4-5
```

needs two new columns on the sessions table: `last_input_tokens` and
`last_context_window`. updated after each provider call via a new
`db.set_session_usage(session_id, input_tokens, context_window)` method.

this is migration v9.

**3. system prompt annotation (agent self-awareness)**

inject a context usage line into the system prompt so the agent can reference it
when asked. appended alongside the existing tool budget section in `system_prompt()`:

```
## context usage
you are currently using approximately 42% of your context window (84000/200000 tokens).
compaction will trigger at 80%. if context is full, suggest starting a new session.
```

this gives the agent honest, grounded information instead of having to guess.
requires passing `last_input_tokens` and `context_window` into `system_prompt()`,
or storing them on the Agent struct.

### warning thresholds

use the existing compaction threshold constant (80%) as the basis:

| range | label | behavior |
|-------|-------|----------|
| 0-60% | normal | log at INFO |
| 60-80% | elevated | log at WARN |
| 80%+ | critical | compaction triggers (existing), log at WARN |

no new user-facing alerts beyond the log level change. the system prompt annotation
gives the agent awareness to proactively mention it if relevant.

### model-aware context capacity

the `context_window()` method already exists on both providers and returns the
correct value per model. no model-to-capacity map is needed — the provider itself
is the source of truth. if a model switch happens mid-conversation, the new
provider's `context_window()` is used automatically (this already works in the
compaction check).

### integration with compaction

no changes to compaction logic itself. the legibility layer is read-only — it
observes the same data compaction uses and makes it visible. the `compacted` and
`compaction_count` fields on ContextUsage are tracked in the agent loop (a simple
counter incremented when compaction runs).

## implementation plan

1. add `ContextUsage` struct to `agent/mod.rs` (or a new `agent/context.rs`)
2. track `compaction_count` in the agent loop (increment after successful compaction)
3. compute `ContextUsage` after each provider call using `response.usage` + `context_window()`
4. replace the two-branch usage log with unified context-aware log, WARN when >60%
5. add migration v9: `last_input_tokens` and `last_context_window` columns on sessions
6. add `db.set_session_usage()` and `db.session_usage()` methods
7. call `set_session_usage()` after each provider call in the agent loop
8. extend `ava status` to read and display context usage + model
9. pass usage info into `system_prompt()` for agent self-awareness
10. add tests for ContextUsage computation and the new db methods

## notes (implementation)

- the system prompt injection means `system_prompt()` needs to accept usage params
  (or the agent tracks them as fields). this is a small signature change.
- the first provider call in a session won't have usage data yet — the system prompt
  should omit the context section when no data is available, and `ava status` should
  show "context: unknown" or similar.
- `compaction_count` is ephemeral (per process_message invocation), not persisted.
  persisting it would require another column but doesn't seem worth it.

full design doc also written to `docs/context-usage-legibility.md` in branch
`ava-palc/context-usage-legibility`.
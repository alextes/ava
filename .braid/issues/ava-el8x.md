---
schema_version: 9
id: ava-el8x
title: graceful tool loop limit with final LLM turn
priority: P1
status: done
deps: []
tags:
- core
- agent
owner: null
created_at: 2026-02-09T16:33:33.942536Z
completed_at: 2026-02-09T17:40:43.976477Z
---

when the agent hits the 20-round tool loop limit, it currently returns `Err(Error::Provider("tool loop exceeded"))`. the user sees a raw error string with no context about what was accomplished. the conversation history ends with tool results the model never responded to.

## current behavior

`agent/mod.rs:132-134`:
```rust
tool_rounds += 1;
if tool_rounds > 20 {
    return Err(Error::Provider("tool loop exceeded".into()));
}
```

this propagates up to `agent_loop` which calls `send_error(sink, "error: provider error: tool loop exceeded")`. problems:

1. **no final LLM turn** — the model never gets to summarize what it did or explain what remains
2. **conversation state is awkward** — the last persisted messages are assistant(tool_use) + user(tool_result) from round 20, with no closing assistant response
3. **no resumability signal** — the user doesn't know they can just send a follow-up to continue
4. **cron-triggered work can't self-continue** — a cron wake that runs out of rounds has no way to schedule continuation

## desired behavior

instead of returning `Err`, give the model one final turn where it understands the situation and can choose how to wrap up. inject a synthetic tool result or user message explaining the limit was reached, then call the provider one last time (without tool definitions, so it can only produce text).

the model should have room to:

- **summarize** what it accomplished in the 20 rounds
- **explain** what work remains
- **ask the user** to send a follow-up message to unlock another 20 rounds
- **schedule continuation** via cron or the task scratchpad (ava-g3o2) if applicable — e.g. a cron agent that can't finish could schedule a follow-up wake

## budget awareness

the model should know about the tool budget at all times, not just when it's exceeded.

### system prompt: declare the budget

add to the system prompt something like:

```
## tool budget
you have a budget of 20 tool rounds per user message. after exhausting the budget, you must produce a final text response. if you need more rounds, ask the user to send a follow-up message to unlock another 20 rounds, or schedule continuation via cron or the task scratchpad.
```

this sets expectations up front so the model can plan its work and pace itself.

### low-budget warning injection

when the model hits 80% usage (round 16 of 20), inject a system-level message into the conversation alongside the tool results. something like:

`"[system: you have used 16 of 20 tool rounds. 4 remain before you must produce a final response. plan accordingly — wrap up, summarize progress, or ask the user to continue.]"`

this gives the model a chance to start wrapping up naturally rather than being cut off abruptly. it can choose to:
- finish what it's doing in the remaining 4 rounds
- produce an early final response summarizing progress
- use a remaining round to schedule a cron or task for continuation

the warning could be a `MessageContent::text` block appended to the tool results message for that round, so the model sees it inline with its normal flow.

## implementation sketch

when `tool_rounds > 20`:

1. don't execute the tool calls from the last provider response
2. persist the assistant message (with its tool_use blocks) as-is
3. construct a synthetic tool result for each pending tool call: `"tool loop limit reached (20 rounds). you must respond to the user now. summarize progress, explain remaining work, and suggest next steps."`
4. persist the synthetic tool results
5. call the provider **one final time** with an empty tools list (no `tools` parameter or `tools: []`) so it can only produce a text response
6. persist and return that final text response as `Ok(Some(outbound))`

passing no tools on the final call ensures the model can't request more tool calls — it must produce a text reply.

## edge case: final turn also fails

if the final provider call itself errors (network, context overflow, etc.), fall back to returning a static message like `"i used all 20 tool rounds for this turn. send a follow-up message and i'll continue where i left off."` as `Ok(Some(...))`, not `Err(...)`.

## files

- `src/agent/mod.rs` — system prompt budget line, low-budget warning injection, replace the `Err` return with the final-turn flow
- `src/provider/mod.rs` — may need a `complete_without_tools` method or the ability to pass an empty tools list
- test: update `test_agent_tool_loop_limit_exceeded` to expect `Ok(Some(...))` instead of `Err`
- test: add test for low-budget warning injection at round 16
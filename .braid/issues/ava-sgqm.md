---
schema_version: 9
id: ava-sgqm
title: 'design: decouple channel constraints from core agent loop'
priority: P2
status: open
type: design
deps: []
tags:
- architecture
- channel
owner: null
created_at: 2026-04-13T11:47:25.907021Z
---

## context

we recently added a telegram message length check (4096-char limit) inside the agent loop in process(). this works — the agent gets feedback and can retry or use send_file — but it means the inner agent loop now knows about telegram-specific constraints.

this is a design smell that will compound as we add more channels (whatsapp, email, slack, web UI, etc.). each channel has its own constraints:
- telegram: 4096-char message limit
- email: no char limit, but attachments work differently
- whatsapp: 4096-char limit, different media handling
- web/CLI: no limit at all

currently the agent loop checks `inbound.channel == ChannelKind::Telegram` before applying the length limit. this breaks separation of concerns.

## desired architecture

the agent should be channel-agnostic. it receives an inbound message (from any source), produces output (text, files, voice), and returns. the agent doesn't know or care whether output goes to telegram, email, or a terminal.

channel-specific constraints should be handled at the channel boundary:
1. agent produces output
2. channel adapter checks constraints (length, attachment format, etc.)
3. if constraints are violated, channel adapter re-enters the agent with feedback
4. agent retries with context about what went wrong

this means the retry loop moves outside process() — the channel adapter calls process(), checks the result, and if needed calls process() again with synthetic feedback. the tradeoff: re-entry is a fresh process() call (reloads messages from DB), but the conversation is persistent so the agent has full context.

## what to explore

- **channel adapter trait**: a trait that wraps process() and handles channel-specific post-processing. each channel implements its own constraints.
- **output validation**: a way for channels to declare their constraints (max message length, supported attachment types, etc.) that the adapter checks.
- **feedback injection**: how to feed "your response was too long" back to the agent without it being a full user message. synthetic system messages? a dedicated re-entry path?
- **send_file generalization**: the send_file tool currently produces a telegram document. for email it'd be an attachment. for CLI it'd just print the path. the tool should be channel-agnostic; the channel adapter translates.
- **proactive hints**: currently the system prompt says "telegram limit is 4096". this should come from the channel adapter, not be hardcoded.

## current state

- length check is in src/agent/mod.rs inside the main loop (the `length_retries` path)
- send_file tool is in src/tool/send_file.rs, produces FileAttachment
- OutboundMessage has an `attachments` field
- send_response in src/queue.rs handles telegram-specific delivery

## output

produce a design that:
1. defines the channel adapter abstraction
2. shows how the retry-on-constraint-violation flow works outside the agent loop
3. handles the system prompt hint injection per-channel
4. is backward compatible with the current telegram flow
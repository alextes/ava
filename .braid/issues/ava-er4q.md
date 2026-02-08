---
schema_version: 9
id: ava-er4q
title: design session architecture
priority: P2
status: done
type: design
deps:
- ava-1q20
tags:
- session
owner: null
created_at: 2026-02-01T21:30:18.670091Z
started_at: 2026-02-08T08:47:22.170869Z
completed_at: 2026-02-08T08:48:04.022575Z
---

ava needs a session mechanism that:

- persists across model switches (claude -> gpt -> claude)
- can continue API provider sessions using their session IDs when available
- can initialize new API sessions with context pulled from storage to continue the abstract conversation
- works across channels (telegram, cli message subcommand, future channels)

ava is intended as a single continuous entity — an identity template/mold someone can instantiate by cloning and giving a new identity.

output: architecture doc covering session storage, context management, and cross-provider continuity.

## design: single active session

### core concept

one global session shared across all channels. telegram and CLI both feed into the same session. no "start a new conversation" — ava is a single continuous entity.

- session is never explicitly closed (for MVP)
- growing window — send all messages from session start (maximizes prompt cache hits, see ava-pvpj)
- compaction deferred to ava-oh2z (not needed for MVP — 50 exchanges ≈ 25k tokens, well within 200k context)
- messages stored in the universal `Message`/`MessageContent` format (provider-agnostic)

### schema changes — v4 migration

the existing v1 tables need minor additions:

```sql
ALTER TABLE sessions ADD COLUMN active INTEGER NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN channel TEXT;
INSERT INTO sessions (active, title) VALUES (1, 'default');
```

no new tables. leave unused `model` column in place (harmless).

### database methods

new methods on `Database`:

- `active_session() -> Result<i64>` — query `WHERE active = 1`, auto-create if missing
- `load_messages(session_id) -> Result<Vec<Message>>` — all messages, oldest-first. content deserialized from JSON
- `append_message(session_id, role, content, channel)` — serialize `Vec<MessageContent>` to JSON, insert, update `sessions.updated_at`

### agent changes

`Agent::process()` flow becomes:

1. `db.active_session()` → session_id
2. `db.load_messages(session_id)` → history (all messages, growing window)
3. `db.append_message(session_id, "user", [user_text], channel)`
4. `messages = history ++ [new_user_msg]`
5. tool loop (same as today, but also persists each assistant/tool_result message as they happen — crash-safe)
6. `db.append_message(session_id, "assistant", [final_response])`
7. return `OutboundMessage`

no message count limit — growing window for prompt cache efficiency (see ava-pvpj). compaction deferred to ava-oh2z.

### message.rs change

add `ChannelKind::as_str()` for storing channel in the messages table.

### what does NOT change

- provider trait — `complete(&self, system_prompt, messages)` stays stateless
- tool system — tools, approval, safety filters unchanged
- facts system — orthogonal to sessions, stays in system prompt
- main.rs — session logic is internal to agent, callers unaffected
- telegram bot — polling loop, callback handling unchanged

### cross-provider continuity

messages stored in universal format. switching providers just means the new provider gets the same `&[Message]` history. provider-specific session IDs deferred — every call sends full history.

### cross-channel continuity

telegram and CLI write to the same session:

1. telegram: "what is 2+2?" → ava: "4"
2. CLI: `ava message "what did I just ask you?"` → ava: "you asked what 2+2 is"

### edge cases

- **concurrent telegram messages**: both load same history, both append. interleaving is slightly odd but harmless. later: per-session mutex to serialize.
- **tool loop crash**: dangling assistant message with tool_use but no tool_result. on load, trim trailing incomplete exchange.
- **very long sessions**: growing window means token count increases over time. compaction (ava-oh2z) will address this. for MVP, context windows are large enough (200k tokens) that this won't be hit in casual use.

### files modified

- `src/db/migrations.rs` — v4 migration
- `src/db/mod.rs` — session/message CRUD methods
- `src/message.rs` — `ChannelKind::as_str()`
- `src/agent/mod.rs` — `process()` loads history + persists messages

implementation: ava-df0i
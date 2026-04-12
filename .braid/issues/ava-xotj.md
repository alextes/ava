---
schema_version: 9
id: ava-xotj
title: 'design: multi-channel awareness in the harness'
priority: P1
status: done
type: design
deps: []
tags:
- telegram
owner: null
created_at: 2026-04-12T09:57:21.834278Z
started_at: 2026-04-12T10:10:38.456538Z
completed_at: 2026-04-12T10:51:52.556688Z
---

figure out how the harness should be aware of and track multiple channels. this is a prerequisite for ren feeling like one entity across channels.

## questions to answer

- what metadata does telegram give us about chats the bot is in? (title, type, member count, description, pinned messages)
- does telegram notify us when the bot is added to / removed from a group? (via update types like `my_chat_member`)
- should we maintain a registry of known channels? where does it live — in memory, in the DB, or derived from ring buffer state?
- how does channel awareness feed into the agent's system prompt? should ren's system prompt list the channels it's in?
- how does this interact with the allowed_ids whitelist? currently we filter by user_id — in group chats, messages come from many users

## scope

- research telegram Bot API docs for chat membership events and metadata
- propose how channel tracking works in the harness
- define what the agent "knows" about its channels
- produce implementation issues

## research findings

### telegram API capabilities

**chat metadata (`getChat`):** returns `ChatFullInfo` — `id`, `type` ("private"/"group"/"supergroup"), `title`, `description`, `photo`, `pinned_message`, `permissions`, `invite_link`. member count requires a separate `getChatMemberCount` call.

**bot membership events (`my_chat_member`):** telegram sends this update whenever the bot's own membership status changes in any chat. contains `chat` (the chat), `from` (who caused it), `old_chat_member` and `new_chat_member` with `status` field: `member`, `administrator`, `left`, `kicked`. currently we subscribe to `["message", "callback_query"]` — need to add `"my_chat_member"`.

**privacy mode:** disabled for ren via BotFather. bot now sees all messages in groups, not just commands/@mentions/replies.

### authorization model (from discussion)

- **DMs:** user whitelist only (`TELEGRAM_ALLOWED_IDS`). non-whitelisted users get an early generic rejection ("DM mode disabled for user ID ...") before any agent processing. no chat_id whitelist needed for DMs.
- **group chats:** chat_id whitelist controls which groups the bot is active in. all users in whitelisted groups can interact. non-whitelisted groups get no response.
- **rejection must be pre-agent:** no LLM sees the message, no prompt injection possible.
- **self-service whitelisting:** whitelisted users in whitelisted chats can ask the bot to add user_ids or chat_ids to the whitelist via a tool.

### whitelist storage

currently: env var `TELEGRAM_ALLOWED_IDS`, parsed at startup, immutable.
needed: persistent, mutable storage for both user_ids and chat_ids. options:

**option A: DB tables.** new `allowed_users` and `allowed_chats` tables. env var seeds initial values, DB is authoritative at runtime. bot can add/remove via tool.
- pro: persistent across restarts, queryable, consistent with how DB is already used
- con: slightly more migration work

**option B: config file.** write to a JSON/TOML file in `~/.ava/`.
- pro: human-editable, no migration
- con: file locking, inconsistent with rest of the system which uses sqlite

**recommendation:** option A. the DB is already the source of truth for all mutable state (sessions, memories, rules, tasks). whitelists belong there too. env var becomes the seed — on startup, merge env var values into DB.

## design

### 1. authorization layer

a new `AuthCheck` enum returned by a function that runs before any agent processing:

```
enum AuthCheck {
    Allowed,                    // proceed to agent
    DmRejected { user_id: i64 },  // send generic rejection, don't process
    ChatNotWhitelisted,         // silently ignore
}
```

the check runs in `telegram_producer` immediately after parsing the message, before the ring buffer or queue:
- **private chat:** check user_id against allowed_users table. reject with message if not found.
- **group/supergroup:** check chat_id against allowed_chats table. silently drop if not found. (no user filtering in groups — being in a whitelisted group is authorization.)
- **this is pre-agent, pre-queue, pre-buffer.** prompt injection in rejected messages has zero attack surface.

### 2. channel registry

a `channels` DB table tracking chats the bot is actively in:

```sql
CREATE TABLE channels (
    chat_id INTEGER PRIMARY KEY,
    chat_type TEXT NOT NULL,  -- "private", "group", "supergroup"
    title TEXT,               -- null for DMs
    added_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

populated via:
- `my_chat_member` events (bot added → insert, bot removed → delete)
- on first message from a new chat_id (upsert with `getChat` metadata)
- `last_seen_at` updated on each message

this is informational — the channel registry does NOT control authorization (that's the whitelist). it tells the agent "here are all the channels i'm in" for system prompt and tool use.

### 3. system prompt channel awareness

add a section to the system prompt listing the channels the bot is active in (whitelisted + has recent activity):

```
## channels
you are active in the following channels:
- #prod-warnings (supergroup, chat_id: -100123) — last active 5m ago
- #engineering (supergroup, chat_id: -100456) — last active 2h ago
- DM with alex (private, chat_id: 789) — last active 1m ago
```

this lets the agent reason about which channel a user might be referring to (e.g. "check what's happening in prod-warnings") and use the `channel_history` tool with the right chat_id.

### 4. self-service whitelist tool

a tool (e.g. `manage_access`) that lets whitelisted users add/remove user_ids and chat_ids:

```
manage_access(action: "add_user" | "remove_user" | "add_chat" | "remove_chat", id: i64)
```

the agent can invoke this when a user says "add user 12345" or "whitelist this chat". the tool itself enforces that the requesting user is already whitelisted — the agent doesn't decide authorization, the tool does.

### 5. `allowed_updates` expansion

change `get_updates` to subscribe to `["message", "callback_query", "my_chat_member"]` so we receive bot membership events.

### planned implementation issues

1. **add allowed_users and allowed_chats DB tables with env var seeding** — new migration, seed from `TELEGRAM_ALLOWED_IDS` + new `TELEGRAM_ALLOWED_CHATS` env var on startup. query methods on Database.
2. **implement pre-agent authorization check in telegram producer** — replace current `allowed_ids.contains()` with DB-backed check. DM rejection message. group silent drop. must be before buffer and queue.
3. **add channels registry table and `my_chat_member` handling** — new migration, subscribe to `my_chat_member` events, upsert on first message via `getChat`.
4. **add channel list to agent system prompt** — query channels table, format active channels into system prompt.
5. **add `manage_access` tool for self-service whitelisting** — tool for adding/removing user_ids and chat_ids. enforces caller is already authorized.
---
schema_version: 9
id: ava-wssp
title: add channel list to agent system prompt
priority: P1
status: done
deps:
- ava-h0fd
tags:
- telegram
owner: null
created_at: 2026-04-12T10:51:42.314789Z
started_at: 2026-04-12T11:39:47.551579Z
completed_at: 2026-04-12T11:41:26.111249Z
---

include a list of active channels in the agent's system prompt so it can reason about cross-channel context.

## scope

- in Agent::system_prompt() (agent/mod.rs), query the channels table and append a channels section:
  ```
  ## channels
  you are active in the following channels:
  - #prod-warnings (supergroup, chat_id: -100123) — last active 5m ago
  - #engineering (supergroup, chat_id: -100456) — last active 2h ago
  - DM with user 789 (private, chat_id: 789) — last active 1m ago
  ```
- format last_seen_at as relative time (e.g. \"5m ago\", \"2h ago\", \"3d ago\")
- only include channels with activity in the last 7 days (avoid stale entries)
- the agent needs access to the DB (it already has it via self.db)

## acceptance criteria

- system prompt includes channel list when channels exist
- channels with no recent activity are excluded
- relative timestamps are human-readable
- DMs show as \"DM with user <id>\", groups show title
- no channel section when no channels are registered (e.g. fresh install)
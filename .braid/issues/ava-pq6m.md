---
schema_version: 9
id: ava-pq6m
title: add manage_access tool for self-service whitelisting
priority: P1
status: done
deps:
- ava-788p
tags:
- telegram
owner: null
created_at: 2026-04-12T10:51:48.483401Z
started_at: 2026-04-12T11:46:50.031738Z
completed_at: 2026-04-12T11:48:26.53826Z
---

let whitelisted users ask the bot to add/remove user_ids and chat_ids from the whitelist.

## scope

- new tool in src/tool/: manage_access
- actions:
  - add_user(user_id) — add a user to allowed_users
  - remove_user(user_id) — remove a user from allowed_users
  - add_chat(chat_id) — add a chat to allowed_chats
  - remove_chat(chat_id) — remove a chat from allowed_chats
  - list — show current allowed users and chats
- the tool itself must verify the requesting user is authorized:
  - the tool receives the chat_id of the current conversation
  - that chat_id must be whitelisted (or be a DM from a whitelisted user)
  - this is enforced in the tool implementation, not by the agent
- users can say things like \"add user 12345\" or \"whitelist this chat\" and the agent invokes the tool
- the tool should confirm what it did (e.g. \"added user 12345 to allowed users\")

## acceptance criteria

- all four mutation actions work correctly
- list action shows current state
- tool rejects requests from non-whitelisted contexts
- changes are immediately effective (next message uses updated whitelist)
- tool is registered and available to the agent
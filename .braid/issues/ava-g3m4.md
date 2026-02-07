---
schema_version: 9
id: ava-g3m4
title: add approval rule management (list/delete)
priority: P2
status: open
deps:
- ava-obhq
tags:
- tool
- approval
owner: null
created_at: 2026-02-07T18:03:12.538034Z
---

users need a way to view and delete approval rules. the DB methods already exist (list_approval_rules, delete_approval_rule) but there's no way to invoke them.

## option A: agent tool

add a `manage_rules` tool the agent can invoke when the user asks about rules:
- `{"action": "list"}` — returns all stored rules
- `{"action": "delete", "id": 123}` — deletes a specific rule

pros: works naturally via conversation ("show me my approval rules", "delete the cargo rule")
cons: another tool in the tool list

## option B: telegram command

respond to `/rules` as a telegram command (not a tool call):
- `/rules` — list all rules
- `/rules delete 1` — delete rule by ID

pros: direct, no LLM roundtrip needed
cons: need command parsing in the telegram loop

either way, this is straightforward. option A is probably better since it's conversational and works across channels.

## files

- `src/tool/mod.rs` — add manage_rules tool definition + handler (if option A)
- or `src/main.rs` — add command parsing (if option B)
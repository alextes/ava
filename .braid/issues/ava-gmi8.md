---
schema_version: 9
id: ava-gmi8
title: wire auto-approval rules into telegram approver
priority: P2
status: open
deps:
- ava-obhq
tags:
- tool
- approval
owner: null
created_at: 2026-02-07T18:03:02.762657Z
---

the TelegramApprover currently always prompts the user for exec commands. it should first check stored approval rules in the database and auto-approve if a matching rule exists.

## what exists

- `Database::find_matching_rule(command)` — checks stored rules against a command
- `Database::save_approval_rule(pattern)` — called from agent when AllowAlways decision is made
- rule matching with wildcard support and pipe/chain decomposition
- approval_rules table (migration v3)

## what's missing

TelegramApprover::request_approval() needs to:
1. extract the command from the tool call input
2. call db.find_matching_rule(command)
3. if a rule matches, return ApprovalDecision::AutoApproved without prompting
4. if no rule matches, proceed with the current keyboard flow

this requires TelegramApprover to have access to the Database. options:
- pass a Database reference to the approver (simplest)
- pass a closure/trait for rule checking

## files

- `src/approver.rs` — add DB access, check rules before prompting
- `src/main.rs` — pass DB to TelegramApprover constructor
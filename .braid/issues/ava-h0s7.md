---
schema_version: 9
id: ava-h0s7
title: skill-secret approval type for telegram
priority: P2
status: done
deps:
- ava-21is
tags:
- secret
- approval
owner: null
created_at: 2026-03-19T08:42:37.033153Z
started_at: 2026-03-20T09:28:40.453765Z
completed_at: 2026-03-24T07:31:12.294765Z
---

extend TelegramApprover with a skill-secret approval type. when a skill with medium-sensitivity secrets is activated, show the user which secrets are being requested and for what skill.

## UX

message format:
> skill **query-prod-db** wants access to:
> • PROD_DB_URL (vault://prod-db-url)
>
> [approve] [deny]

no "always" button — secret access is always per-activation.

## implementation

- add a new approval variant in approver.rs for skill secrets
- the approval groups all medium-sensitivity secrets for a single skill activation
- on approve, return the list of approved secret names
- on deny, skill activation fails with a clear message
- timeout: same 5-minute window as command approval
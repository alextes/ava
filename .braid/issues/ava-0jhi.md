---
schema_version: 9
id: ava-0jhi
title: add action=add to manage_rules tool with approval flow
priority: P2
status: done
deps: []
tags:
- tool
- approval
owner: null
created_at: 2026-02-08T20:24:51.530918Z
started_at: 2026-02-08T20:24:56.885818Z
completed_at: 2026-02-08T20:27:41.190544Z
---

extend manage_rules with action=add for agent-proposed approval rules. requires_approval() triggers for add actions, TelegramApprover shows the proposed pattern for human review.
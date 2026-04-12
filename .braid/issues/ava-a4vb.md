---
schema_version: 9
id: ava-a4vb
title: add user-explicit skill invocation
priority: P2
status: done
deps:
- ava-9smj
tags:
- core
- skill
owner: null
created_at: 2026-02-10T13:36:13.692231Z
started_at: 2026-03-19T12:09:41.502263Z
completed_at: 2026-03-19T12:14:43.331933Z
---

detect /skill-name prefix in incoming user messages. if a matching skill exists (and user-invocable is true), prepend the skill body to the message wrapped in [skill: name]...[/skill] tags. support $ARGUMENTS substitution — everything after the skill name replaces $ARGUMENTS in the body. handle in the agent layer before sending to the LLM.
---
schema_version: 9
id: ava-k1vz
title: inject skill descriptions into system prompt
priority: P2
status: open
deps:
- ava-9smj
tags:
- core
- skill
owner: null
created_at: 2026-02-10T13:36:03.967661Z
---

add an '## available skills' section to the system prompt listing name + description for all model-invocable skills (where disable-model-invocation is false). cap the section at 2000 chars. wire the skill list into Agent so system_prompt() can access it.
---
schema_version: 9
id: ava-mo38
title: add activate_skill tool
priority: P2
status: done
deps:
- ava-9smj
tags:
- tool
- skill
owner: null
created_at: 2026-02-10T13:36:08.24487Z
started_at: 2026-03-19T12:20:58.55825Z
completed_at: 2026-03-19T12:24:46.045182Z
---

add a new activate_skill tool the LLM can call to load a skill's full instructions. input: { name: string }. returns the skill body as the tool result. the LLM then follows those instructions. add tool definition + handler in src/tool/. wire the skill list into the tool dispatch so it can look up skills by name.
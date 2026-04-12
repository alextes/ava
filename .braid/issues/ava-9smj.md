---
schema_version: 9
id: ava-9smj
title: add skill loading from ~/.ava/skills/
priority: P2
status: done
deps: []
tags:
- core
- skill
owner: null
created_at: 2026-02-10T13:35:59.721764Z
started_at: 2026-03-19T08:20:59.538037Z
completed_at: 2026-03-19T08:25:08.974249Z
---

scan ~/.ava/skills/*/SKILL.md on startup. parse YAML frontmatter (name, description, user-invocable, disable-model-invocation) and markdown body. store as Vec<Skill> in memory. create the Skill struct. use a simple YAML parser (serde_yaml or yaml-rust2). no hot-reloading — restart to pick up new skills.
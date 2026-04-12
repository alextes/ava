---
schema_version: 9
id: ava-wum2
title: add ava skills CLI subcommand
priority: P2
status: done
deps:
- ava-9smj
tags:
- skill
owner: null
created_at: 2026-02-10T13:36:17.876278Z
started_at: 2026-03-19T12:26:44.938696Z
completed_at: 2026-03-19T12:28:43.245992Z
---

add `ava skills` subcommand that lists installed skills (name + description). reads from ~/.ava/skills/. simple table output. helps users see what's available without checking the filesystem.
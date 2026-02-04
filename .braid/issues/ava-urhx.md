---
schema_version: 9
id: ava-urhx
title: design sub-agent spawning
priority: P3
status: open
type: design
deps: []
tags:
- core
owner: null
created_at: 2026-02-04T21:37:23.496888Z
---

recursive agent delegation for complex multi-step tasks.

## aidaemon approach
- agent can spawn child agents for subtasks
- configurable depth limit
- session ID tracking: "sub-{depth}-{uuid}"
- child inherits tools but has own conversation

## questions to consider
- when should agent spawn vs handle inline?
- resource limits per sub-agent?
- communication between parent and child?
- shared vs isolated memory?
- depth limits to prevent runaway spawning?
- cost attribution to parent task?

## output
- spawning interface
- resource/depth limits
- result aggregation
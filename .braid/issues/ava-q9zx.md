---
schema_version: 9
id: ava-q9zx
title: add tests for agent error propagation
priority: P2
status: done
deps: []
tags:
- test
owner: null
created_at: 2026-02-01T22:57:53.128534Z
started_at: 2026-02-02T08:40:55.22961Z
completed_at: 2026-02-04T20:36:59.554651Z
---

the agent test only covers the happy path. add tests for:

- provider returns error → agent propagates it
- channel send fails → agent propagates it

use MockProvider/MockChannel that return errors.
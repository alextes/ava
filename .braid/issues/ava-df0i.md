---
schema_version: 9
id: ava-df0i
title: implement session mechanism
priority: P2
status: open
deps:
- ava-1q20
tags:
- session
owner: null
created_at: 2026-02-01T21:30:22.897441Z
---

implement the session mechanism based on the design (ava-er4q):

- session storage in sqlite
- context retrieval and injection for new API sessions
- session continuity across channels and providers

depends on both the design being complete and sqlite being set up.
---
schema_version: 9
id: ava-rpr9
title: rich progress messages during agent processing
priority: P2
status: open
type: design
deps:
- ava-l3dy
owner: null
created_at: 2026-04-13T10:07:03.779113Z
---

explore approaches for showing detailed status messages in telegram during long agent turns (tool round counts, compaction state). the typing indicator (ava-l3dy) covers the quick win — this issue is about the richer edit-in-place approach. compare: progress callback on agent.process(), shared state + polling, channel-based sender, or other patterns. key question: how to thread progress info out of the encapsulated agent loop without coupling agent to telegram.
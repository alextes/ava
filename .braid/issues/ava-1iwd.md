---
schema_version: 9
id: ava-1iwd
title: add tracing for structured logging
priority: P2
status: open
deps: []
tags:
- observability
owner: null
created_at: 2026-02-01T23:06:11.089989Z
---

add the tracing ecosystem for structured logging throughout ava.

dependencies:
- tracing
- tracing-subscriber (with fmt and env-filter features)

initialize in main.rs with env filter (RUST_LOG). add spans/events for:
- agent processing messages
- provider API calls (request/response, timing)
- errors

keep it minimal initially, expand as needed.
---
schema_version: 9
id: ava-fmgy
title: persist context usage to sessions table (migration v9)
priority: P2
status: done
deps:
- ava-yquw
- ava-oh2z
tags:
- session
- observability
owner: null
created_at: 2026-03-15T22:20:59.020352Z
started_at: 2026-03-15T22:31:29.080399Z
completed_at: 2026-03-15T22:33:15.865596Z
---

add migration v9: last_input_tokens and last_context_window columns on sessions table. add db.set_session_usage(session_id, input_tokens, context_window) and db.session_usage() methods. call set_session_usage() after each provider call in the agent loop.
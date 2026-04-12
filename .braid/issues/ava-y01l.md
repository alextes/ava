---
schema_version: 9
id: ava-y01l
title: fix telegram BUTTON_DATA_INVALID when approval pattern exceeds 64 bytes
priority: P1
status: done
deps: []
tags:
- approval
- telegram
owner: null
created_at: 2026-03-13T10:43:28.12977Z
completed_at: 2026-03-13T10:43:31.025831Z
---

callback_data for 'always' buttons embedded the full pattern string, which exceeded telegram's 64-byte limit for inline keyboard callback data. long paths (e.g. edit:/Users/.../src/commands/**) pushed the total past the limit, causing Bad Request: BUTTON_DATA_INVALID errors.

fix: store patterns in a vec on PendingApproval and reference by index in callback_data (always:0, always:1, etc.).
---
schema_version: 9
id: ava-vg4w
title: add cron tool
priority: P2
status: done
deps:
- ava-b13x
- ava-1q20
- ava-lzbb
tags:
- tool
owner: null
created_at: 2026-02-01T21:37:25.61852Z
started_at: 2026-02-09T08:38:08.254207Z
completed_at: 2026-02-09T16:37:50.827496Z
---

scheduling tool for the agent loop:

- schedule one-time future events
- schedule recurring tasks
- persist schedules in sqlite
- trigger agent actions when scheduled time arrives

lets ava plan ahead and maintain routines.
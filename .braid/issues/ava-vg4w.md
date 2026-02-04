---
schema_version: 9
id: ava-vg4w
title: add cron tool
priority: P2
status: open
deps:
- ava-b13x
- ava-1q20
tags:
- tool
owner: null
created_at: 2026-02-01T21:37:25.61852Z
---

scheduling tool for the agent loop:

- schedule one-time future events
- schedule recurring tasks
- persist schedules in sqlite
- trigger agent actions when scheduled time arrives

lets ava plan ahead and maintain routines.
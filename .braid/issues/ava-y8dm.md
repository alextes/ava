---
schema_version: 9
id: ava-y8dm
title: add telegram plain text resend fallback
priority: P2
status: open
deps:
- ava-91j8
tags:
- telegram
owner: null
created_at: 2026-02-01T21:30:12.77895Z
---

if telegram rejects a message due to HTML parsing errors, automatically resend as plain text. graceful degradation over failed delivery.
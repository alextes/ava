---
schema_version: 9
id: ava-b13x
title: implement agent loop
priority: P2
status: done
deps:
- ava-bhwz
tags:
- core
owner: null
created_at: 2026-02-01T21:37:10.67757Z
completed_at: 2026-02-01T23:02:06.774224Z
---

the core agent loop:

- receives messages from channels (telegram, cli message subcommand, etc.)
- calls the provider API
- handles tool calls from the model response
- returns results back to the channel

this is the heart of ava — where messages come in and intelligence happens.
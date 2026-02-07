---
schema_version: 9
id: ava-obhq
title: add exec tool
priority: P2
status: done
deps:
- ava-b13x
tags:
- tool
owner: null
created_at: 2026-02-01T21:37:16.273137Z
completed_at: 2026-02-07T18:02:50.101939Z
---

powerful command execution tool for the agent loop:

- by default runs commands in a docker container
- can run on host system when needed
- filter obvious malicious commands before execution
- return stdout, stderr, exit code

security-sensitive — needs careful filtering and sandboxing.
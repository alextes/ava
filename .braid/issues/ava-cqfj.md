---
schema_version: 9
id: ava-cqfj
title: add ava logs subcommand
priority: P2
status: done
deps:
- ava-pne5
tags:
- daemon
owner: null
created_at: 2026-02-09T17:47:03.683591Z
started_at: 2026-03-17T20:24:55.812157Z
completed_at: 2026-03-17T20:38:25.955602Z
---

add `ava logs` subcommand. tails ~/.ava/ava.log (equivalent to tail -f). depends on daemonize since logs only go to file after daemonization.
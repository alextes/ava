---
schema_version: 9
id: ava-cqfj
title: add ava logs subcommand
priority: P2
status: open
deps:
- ava-pne5
tags:
- daemon
owner: null
created_at: 2026-02-09T17:47:03.683591Z
---

add `ava logs` subcommand. tails ~/.ava/ava.log (equivalent to tail -f). depends on daemonize since logs only go to file after daemonization.
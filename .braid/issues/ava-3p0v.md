---
schema_version: 9
id: ava-3p0v
title: split large modules (main, agent, tool)
priority: P2
status: done
deps: []
tags:
- refactor
owner: null
created_at: 2026-03-09T14:43:10.743563Z
completed_at: 2026-03-09T19:55:13.888742Z
---

main.rs (926 lines), agent/mod.rs (1417 lines), and tool/mod.rs (1161 lines) are getting unwieldy.

split plan:
- main.rs: extract CLI structs to src/cli.rs, move subcommand impls to src/commands/ (start.rs, upgrade.rs, doctor.rs, history.rs, message.rs). main.rs stays thin — just main() dispatching.
- agent/mod.rs: extract system prompt building helpers to agent/prompt.rs
- tool/mod.rs: extract remember/recall/forget handlers to tool/memory.rs
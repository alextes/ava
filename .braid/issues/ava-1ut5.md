---
schema_version: 9
id: ava-1ut5
title: design terminal tool with approval flow
priority: P1
status: open
type: design
deps: []
tags:
- tool
owner: null
created_at: 2026-02-04T21:37:05.077966Z
started_at: 2026-02-06T22:11:05.03143Z
---

shell command execution with safety controls.

## aidaemon approach
- runs via `sh -c`, no sandboxing
- auto-approve safe commands (ls, cat, grep, etc.)
- always require approval for shell operators (; | && || $())
- telegram buttons: Allow Once / Allow Always / Deny
- "Allow Always" persists to SQLite
- output truncated to 4000 chars

## questions to consider
- sandboxing options? docker, nsjail, firejail?
- allowlist vs blocklist approach?
- should approval be per-command or per-pattern?
- timeout handling for long-running commands?
- working directory management?
- environment variable exposure?
- how to handle interactive commands?

## output
- security model design
- approval flow UX
- tool interface
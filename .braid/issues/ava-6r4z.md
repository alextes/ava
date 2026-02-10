---
schema_version: 9
id: ava-6r4z
title: add ~/.ava/ directory and PID file management
priority: P2
status: open
deps: []
tags:
- daemon
owner: null
created_at: 2026-02-09T17:46:51.19838Z
---

create the ~/.ava/ directory on startup if it doesn't exist. add PID file utilities: write_pid_file(), read_pid_file(), check_process_alive(pid). use kill(pid, 0) to check liveness. this is the foundation for all daemon subcommands.
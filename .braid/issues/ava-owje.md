---
schema_version: 9
id: ava-owje
title: add tests for config path resolution
priority: P2
status: done
deps: []
tags:
- test
owner: null
created_at: 2026-02-01T22:57:51.583548Z
started_at: 2026-02-01T23:07:15.115707Z
completed_at: 2026-02-04T20:36:59.540639Z
---

config.rs has no tests. the most important logic to test:

- `default_db_path()` returns AVA_DB_PATH env var when set
- `default_db_path()` falls back to data_dir when env var not set
- `data_dir()` returns error when no home directory

use temp_env or similar to test env var handling.
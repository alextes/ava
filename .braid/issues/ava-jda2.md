---
schema_version: 9
id: ava-jda2
title: move sqlite db to repo
priority: P1
status: done
deps: []
tags:
- storage
owner: null
created_at: 2026-02-04T21:49:06.26126Z
started_at: 2026-02-04T21:59:51.774434Z
completed_at: 2026-02-04T22:01:16.736681Z
---

for now, keep the sqlite db in the repo root (ava.db) instead of the system data directory.

simpler for development, easy to inspect, backs up with the repo.

update config::default_db_path() to return ./ava.db and add ava.db to .gitignore.
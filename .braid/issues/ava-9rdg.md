---
schema_version: 9
id: ava-9rdg
title: 'design: parallel CI runners with per-runner persistent caches'
priority: P2
status: open
type: design
deps: []
tags:
- ci
owner: null
created_at: 2026-02-10T14:15:22.870974Z
---

the self-hosted runner currently executes all jobs sequentially. because it uses host dir mounting for the cargo target cache, running jobs in parallel would cause cache corruption or build races.

**question:** can we run multiple runners (or docker-based builders) on the same machine where each has its own isolated filesystem with a persistent cache, so jobs that are independent according to the github action DAG actually run in parallel while still getting large caching benefits?

**things to investigate:**
- multiple github runner instances with separate work dirs
- docker-in-docker or sysbox runners with volume-mounted caches
- buildkit / nix-based caching that handles concurrent access
- whether the time savings justify the added complexity
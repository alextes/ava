---
schema_version: 9
id: ava-z9g6
title: add tests for migration data preservation
priority: P2
status: done
deps: []
tags:
- test
- db
owner: null
created_at: 2026-02-08T18:43:31.664438Z
completed_at: 2026-02-08T18:47:56.520117Z
---

no test verifies that existing facts rows survive the v6 migration into the memories table. the migration runs in tests (verified by schema version), but data integrity isn't checked.

## what to test

- insert facts at schema v5 (directly into facts table)
- run v6 migration
- verify memories table contains migrated facts with correct kind, content, category, key
- verify fts5 index contains migrated content (searchable via search_memories)
- verify facts table no longer exists

## approach

create a test that opens an in-memory DB, manually runs migrations 1-5, inserts test data into facts, then runs migration 6, and verifies the data in memories.

## files

- `src/db/mod.rs` — add migration data preservation test
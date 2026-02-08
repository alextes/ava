---
schema_version: 9
id: ava-pwyl
title: 'design: rusqlite vs sqlx comparison'
priority: P2
status: open
type: design
deps: []
tags:
- db
owner: null
created_at: 2026-02-08T14:54:53.146168Z
---

compare rusqlite (current) vs sqlx for ava's database layer.

## context

ava currently uses rusqlite with a std::sync::Mutex wrapper to make the connection Sync. this works but has trade-offs worth evaluating.

## research questions

- async support: sqlx is natively async, rusqlite requires blocking in async context (mutex lock). how much does this matter in practice?
- migration system: sqlx has built-in compile-time checked migrations. rusqlite uses our hand-rolled migration runner. is the sqlx approach better?
- connection pooling: sqlx has built-in pooling. rusqlite is single-connection. does ava need pooling?
- type safety: sqlx checks queries at compile time. rusqlite is runtime-only. how much does this help?
- bundle size / compile time: rusqlite bundles sqlite. sqlx links dynamically or uses feature flags. impact?
- ecosystem: which is more actively maintained? better docs?
- migration path: how much work to switch? is it worth it given what we have works?

## output

recommendation on whether to switch, stay, or revisit later.
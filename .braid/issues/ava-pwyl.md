---
schema_version: 9
id: ava-pwyl
title: 'design: rusqlite vs sqlx comparison'
priority: P2
status: done
type: design
deps: []
tags:
- db
owner: null
created_at: 2026-02-08T14:54:53.146168Z
completed_at: 2026-02-21T15:23:48.507346Z
---

compare rusqlite (current) vs sqlx for ava's database layer.

## context

ava uses `rusqlite 0.33` with `bundled` feature and a `std::sync::Mutex<Connection>` wrapper. the DB layer has 6 module files, 8 migrations, ~35 `.conn.lock().unwrap()` call sites, and is used from async tokio context throughout (agent loop, approver, scheduler, tools, main).

## research findings

### async support

**rusqlite**: synchronous. every DB call acquires the mutex, blocks the current thread. in ava's tokio runtime, this means a DB call on an async task blocks that executor thread until the query completes.

**sqlx**: natively async. queries yield the executor thread while waiting on I/O.

**does it matter for ava?** not much. sqlite is an embedded database — there's no network I/O. queries hit the local filesystem and typically complete in microseconds to low milliseconds. the mutex hold time is extremely short. ava is not a high-concurrency web server; it's a single-user personal assistant. the theoretical blocking is negligible in practice.

### migration system

**rusqlite (current)**: hand-rolled `MIGRATIONS` array in `migrations.rs`. 30 lines of code. simple, linear, versioned. works well.

**sqlx**: compile-time checked migrations via `sqlx migrate!()` macro. requires a running database at compile time (or `SQLX_OFFLINE=true` with a cached query map). adds a build step and CI complexity.

**verdict**: ava's migration runner is simple and sufficient. sqlx's migration system adds compile-time database dependency which is annoying for CI and fresh clones.

### connection pooling

**sqlx**: built-in connection pool via `sqlx::SqlitePool`.

**rusqlite**: single connection behind a mutex.

**does ava need pooling?** no. ava is single-user, single-process. a single connection is fine. sqlite itself serializes writes anyway (WAL mode helps with concurrent reads, but ava doesn't have concurrent read pressure). pooling adds complexity for zero benefit here.

### type safety / compile-time query checking

**sqlx**: `sqlx::query!()` checks SQL syntax and column types at compile time. catches typos and schema drift before runtime.

**rusqlite**: runtime-only. SQL errors surface when the code runs.

**how much does this help?** it's a genuine advantage of sqlx. however, ava has good test coverage — every DB module has tests that exercise queries against an in-memory database. compile-time checking would catch issues slightly earlier (compile vs test), but the existing test suite already provides strong coverage.

### bundle size / compile time

**rusqlite (bundled)**: bundles sqlite3 C source. compiles it as part of the build. adds ~30s to clean builds. no runtime dependency on system sqlite.

**sqlx (sqlite)**: uses `libsqlite3-sys` under the hood (same bundling approach), OR can link to system sqlite. compile times are similar or worse due to sqlx's proc macros and the compile-time query checking machinery.

**verdict**: roughly equivalent. sqlx may be slightly slower to compile due to macro expansion.

### ecosystem and maintenance

**rusqlite**: mature, well-maintained, 3.5k+ GitHub stars. straightforward wrapper around sqlite3 C API. stable API.

**sqlx**: well-maintained, 14k+ GitHub stars. broader scope (postgres, mysql, sqlite). more complex codebase. occasional breaking changes between major versions.

both are solid choices. rusqlite is more focused and simpler.

### migration path

switching would require:
- replacing `Mutex<Connection>` with `SqlitePool`
- rewriting all ~35 call sites from synchronous `conn.lock().unwrap()` + `conn.execute/query_row` to async `sqlx::query!()` / `sqlx::query_as!()`
- converting all `impl Database` methods to `async fn`
- converting all callers to `.await` the DB calls
- rewriting the migration runner or converting to sqlx's migration format
- updating all ~80+ test functions
- adding `SQLX_OFFLINE` / `sqlx-data.json` for CI

estimated effort: 1-2 days of mechanical refactoring. medium risk of introducing bugs during the transition.

## recommendation: stay with rusqlite

the current setup works well for ava's use case:

1. **ava is single-user, single-connection** — pooling and async I/O provide no practical benefit for an embedded sqlite database
2. **the existing code is clean and well-tested** — 35 call sites, good test coverage, simple migration runner
3. **the migration cost is non-trivial** — ~80+ test functions and ~35 call sites to rewrite, plus CI changes
4. **compile-time query checking is nice but not critical** — the test suite already catches query issues
5. **rusqlite's simplicity matches ava's needs** — less abstraction, fewer dependencies, simpler mental model

**revisit if**: ava needs to support postgres/mysql (multi-backend), or moves to a multi-process architecture where connection pooling matters, or the blocking becomes measurable (profile first).
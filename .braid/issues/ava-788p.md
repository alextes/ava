---
schema_version: 9
id: ava-788p
title: add allowed_users and allowed_chats DB tables with env var seeding
priority: P1
status: done
deps: []
tags:
- telegram
owner: null
created_at: 2026-04-12T10:51:15.803431Z
started_at: 2026-04-12T10:52:26.129643Z
completed_at: 2026-04-12T10:55:26.375752Z
---

new DB tables for persistent, mutable user and chat whitelists. env vars seed initial values, DB is authoritative at runtime.

## scope

- new migration adding two tables:
  ```sql
  CREATE TABLE allowed_users (
      user_id INTEGER PRIMARY KEY,
      added_at TEXT NOT NULL DEFAULT (datetime('now')),
      added_by TEXT  -- "env" for seed, or user_id of who added them
  );
  CREATE TABLE allowed_chats (
      chat_id INTEGER PRIMARY KEY,
      added_at TEXT NOT NULL DEFAULT (datetime('now')),
      added_by TEXT
  );
  ```
- on startup (in run_start), seed from env vars:
  - `TELEGRAM_ALLOWED_IDS` → insert into allowed_users (skip if already exists)
  - `TELEGRAM_ALLOWED_CHATS` (new env var, comma-separated chat_ids) → insert into allowed_chats
- Database methods:
  - `is_user_allowed(user_id: i64) -> bool`
  - `is_chat_allowed(chat_id: i64) -> bool`
  - `add_allowed_user(user_id: i64, added_by: &str)`
  - `remove_allowed_user(user_id: i64)`
  - `add_allowed_chat(chat_id: i64, added_by: &str)`
  - `remove_allowed_chat(chat_id: i64)`
  - `list_allowed_users() -> Vec<i64>`
  - `list_allowed_chats() -> Vec<i64>`
- DMs should bypass chat whitelist entirely — only user whitelist matters for private chats

## acceptance criteria

- tables created via migration
- env var seeding works on first startup and is idempotent on subsequent startups
- all query/mutation methods work and are tested
- existing TELEGRAM_ALLOWED_IDS behavior preserved (just backed by DB now)
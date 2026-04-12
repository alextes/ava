---
schema_version: 9
id: ava-j1ye
title: restrict filesystem tool to working directory by default
priority: P2
status: done
deps: []
tags:
- security
- tool
owner: null
created_at: 2026-03-12T13:35:42.879395Z
started_at: 2026-03-16T13:13:59.520133Z
completed_at: 2026-03-16T13:25:45.720887Z
---

the filesystem tool should operate read-only within the directory ava is started from by default. expanding access requires explicit permission.

## motivation

running ava on a server or exposed to the internet means filesystem access should be least-privilege by default. currently the filesystem tool can read/write anywhere on the system the process has access to, which is too broad.

## design

### default: read-only within working directory

- on startup, record the working directory as the 'workspace root'
- filesystem tool read operations (view, grep, glob) are allowed within workspace root without approval
- filesystem tool write operations (create, str_replace, insert) require approval by default, even within workspace root (edit: rules can pre-approve specific paths as today)
- any access outside workspace root requires explicit permission request

### permission expansion

when access is blocked, the user gets three options:

1. **allow once** — permit this specific access, no rule saved
2. **allow always** — save a persistent rule for the path's subtree, scoped to the permission level being requested:
   - read request → saves `read:/path/**` (only grants read)
   - write request → saves `edit:/path/**` (grants read + write)
3. **deny** — block the access

two levels of expansion:

1. **read access to a new path** — 'ava is requesting read access to /Users/alex/documents/'
2. **read/write access to a new path** — 'ava is requesting read/write access to /etc/'

an existing `edit:` rule for a path implies read access to that path (no need for a separate `read:` rule).

### absolute path behaviour

- absolute paths outside workspace: block and request permission
- absolute paths inside workspace: allow per existing edit rules
- symlinks: resolve and check the real path

## notes

- the workspace root should be configurable via env var (AVA_WORKSPACE or similar)
- this composes with the existing edit: rule namespace
- pairs well with ava-n07t (prompt injection detection) as a defence-in-depth layer
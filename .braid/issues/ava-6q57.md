---
schema_version: 9
id: ava-6q57
title: hard-deny vault directory reads in tool layer
priority: P2
status: done
deps: []
tags:
- security
- secret
owner: null
created_at: 2026-03-19T08:42:08.708104Z
started_at: 2026-03-20T09:09:57.046279Z
completed_at: 2026-03-20T09:20:11.422735Z
---

add ~/.ava/vault/ as a hardcoded deny path in the tool layer. no approval rules can grant read access. this applies to text_editor view, grep, glob, and any other tool that reads files.

## approach

add a check in requires_approval() or the workspace boundary logic that returns a hard deny (not approval-gatable) for any path under ~/.ava/vault/. this is similar to the safety filter that blocks rm -rf / — it's a non-negotiable boundary.

## files to modify

- src/tool/workspace.rs — add vault path check
- src/tool/mod.rs — wire hard deny into requires_approval or handle separately

## acceptance criteria

- agent cannot read files under ~/.ava/vault/ via any tool
- agent cannot glob or grep files under ~/.ava/vault/
- approval rules like read:~/.ava/** do not override the vault deny
- attempting to read vault files returns a clear error message
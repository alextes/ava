---
schema_version: 9
id: ava-jahg
title: implement filesystem operations and tool dispatch
priority: P2
status: doing
deps:
- ava-o6ip
tags:
- tool
owner: alextes
created_at: 2026-02-07T19:12:58.041404Z
started_at: 2026-02-08T21:49:01.154445Z
---

implement the filesystem tool end-to-end:

1. **filesystem operations module** (`src/tool/filesystem.rs`):
   - `read_file(path, line_range)` — read file contents, optional line range, return with line numbers
   - `write_file(path, content)` — create or overwrite a file
   - `str_replace(path, old_str, new_str)` — exact string replacement, must match exactly one location
   - `insert(path, line_number, text)` — insert text after a specific line
   - `list_dir(path)` — list directory contents
   - path validation: resolve symlinks, reject `..` traversal, validate against allowed directories
   - safety: block writes to sensitive files (.env, credentials, etc.)
   - output truncation for large files

2. **tool dispatch** in `handle_tool_call`:
   - match on `str_replace_based_edit_tool` (anthropic's built-in tool name)
   - parse the `{command, path, old_str, new_str, ...}` input format
   - route each command to the appropriate fs operation
   - return actionable error messages (no match found, multiple matches, file not found)

3. **approval**:
   - `str_replace`, `create`, `insert` → require approval (like exec)
   - `view` → auto-approve (read-only)
   - extend `requires_approval()` to cover filesystem write operations
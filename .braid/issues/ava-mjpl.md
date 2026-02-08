---
schema_version: 9
id: ava-mjpl
title: add cwd parameter to exec tool
priority: P2
status: done
deps: []
tags:
- tool
owner: null
created_at: 2026-02-08T21:44:06.668158Z
started_at: 2026-02-08T21:46:35.403315Z
completed_at: 2026-02-08T21:47:26.864153Z
---

add an optional `cwd` parameter to the exec tool so the model can run commands in a specific directory without `cd dir && command` boilerplate.

### why

ava spawns a fresh `sh -c` process per command (no persistent shell), so `cd` does not persist between calls. without a cwd param, the model has to prefix every command with `cd /some/dir &&`. codex solves this with an explicit `workdir` param.

### implementation

1. add `cwd: Option<String>` to `ExecInput` in `src/tool/mod.rs`
2. in `execute_command()`, pass cwd to `tokio::process::Command`:
   ```rust
   let mut cmd = tokio::process::Command::new("sh");
   cmd.arg("-c").arg(command);
   if let Some(dir) = &cwd {
       cmd.current_dir(dir);
   }
   ```
3. update `exec_definition()` JSON schema to include cwd:
   ```json
   "cwd": { "type": "string", "description": "working directory for the command (default: process working directory)" }
   ```
4. update tool description to mention the cwd parameter
5. add tests: exec with valid cwd, exec with nonexistent cwd (should return error)

### scope

small change — one file, one struct field, one line in execute_command, schema update, tests.
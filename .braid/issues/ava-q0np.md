---
schema_version: 9
id: ava-q0np
title: design grep and glob tools for codebase navigation
priority: P2
status: done
type: design
deps: []
tags:
- tool
- search
owner: null
created_at: 2026-02-11T09:09:16.126353Z
started_at: 2026-02-11T09:09:25.026589Z
completed_at: 2026-02-11T10:27:05.587203Z
---

design grep and glob search tools so the agent can efficiently navigate and search codebases it works on.

## context

claude code gives agents three text-based search tools (grep, glob, read) backed by ripgrep — no embeddings, no vector DB. boris cherny (head of claude code) confirmed they dropped RAG because agentic grep/glob "outperformed everything, by a lot" while avoiding index staleness, security, and complexity issues.

source: https://x.com/bcherny/status/2017824286489383315

## decision: two tools, not one

two separate tools: `grep` (search file contents) and `glob` (find files by name pattern). keeps each tool focused and matches claude code's design. the model uses glob to discover files, grep to search contents, and read (existing `text_editor view`) for full file reads.

## decision: shell out to `rg` for grep

shell out to `rg --json` rather than using the `grep` crate family as a library.

**why:**
- zero new compile dependencies (uses `tokio::process::Command`, already in tree)
- `rg --json` gives structured output with line numbers, context, file paths, binary detection, encoding handling — all for free
- `rg` respects `.gitignore` by default — critical for avoiding `target/`, `node_modules/`, etc.
- process spawn overhead (~1-5ms) is negligible compared to LLM round-trip latency
- the `grep` crate family requires composing 5 sub-crates with sparse documentation and would add ~6 new dependencies

**risk:** `rg` must be installed. mitigate with a clear error message ("rg not found — install ripgrep: cargo install ripgrep"). `rg` is near-universal on developer machines.

## decision: `ignore` crate for glob

use the `ignore` crate (from ripgrep ecosystem) rather than the `glob` crate.

**why:**
- respects `.gitignore` (nested, global, `.git/info/exclude`) — essential for avoiding noise
- `WalkBuilder` API is well-designed with parallel walking support
- no good "shell out" equivalent for glob (unlike grep where `rg --json` is excellent)

**tradeoff:** adds ~6 new crates (`globset`, `aho-corasick`, `bstr`, `walkdir`, `crossbeam-deque`, `same-file`). `regex-automata` and `memchr` are already in tree.

**alternative considered:** shell out to `rg --files -g 'pattern'` for zero deps. viable but more awkward than the library approach, and glob patterns need to go through shell escaping.

## grep tool design

```json
{
  "name": "grep",
  "description": "search file contents using regex. powered by ripgrep.",
  "input_schema": {
    "type": "object",
    "properties": {
      "pattern": {
        "type": "string",
        "description": "regex pattern to search for"
      },
      "path": {
        "type": "string",
        "description": "file or directory to search in (default: working directory)"
      },
      "glob": {
        "type": "string",
        "description": "filter files by glob pattern, e.g. '*.rs'"
      },
      "context_lines": {
        "type": "integer",
        "description": "number of lines to show before and after each match (default: 0)"
      },
      "max_results": {
        "type": "integer",
        "description": "maximum number of matching lines to return (default: 50)"
      },
      "case_insensitive": {
        "type": "boolean",
        "description": "case insensitive search (default: false)"
      }
    },
    "required": ["pattern"]
  }
}
```

implementation: build `rg` command args, invoke via `tokio::process::Command`, parse `--json` output, format as text for the model.

output format — numbered lines with file paths:
```
src/tool/mod.rs:
  42: pub struct ToolCall {
  43:     pub id: String,
  44:     pub name: String,

src/tool/exec.rs:
  10: const EXEC_TOOL_NAME: &str = "exec";
```

truncation: 4000 chars (matching existing tool limits). include a note like `(truncated, 247 more matches)` when truncated.

## glob tool design

```json
{
  "name": "glob",
  "description": "find files by name pattern. respects .gitignore.",
  "input_schema": {
    "type": "object",
    "properties": {
      "pattern": {
        "type": "string",
        "description": "glob pattern, e.g. '**/*.rs' or 'src/**/*.toml'"
      },
      "path": {
        "type": "string",
        "description": "directory to search in (default: working directory)"
      }
    },
    "required": ["pattern"]
  }
}
```

implementation: use `ignore::WalkBuilder` to walk directory, `globset::Glob` to match entries, collect paths, sort by modification time (newest first).

output format — one path per line:
```
src/tool/mod.rs
src/tool/exec.rs
src/tool/web.rs
src/tool/filesystem.rs
```

truncation: 4000 chars, with `(truncated, 83 more files)` note.

## approval

neither tool requires approval — they are read-only. matches the existing pattern where `recall` and `text_editor view` don't require approval.

## implementation plan

1. add `ignore` and `globset` to `Cargo.toml`
2. create `src/tool/search.rs` with `grep_definition()`, `glob_definition()`, `handle_grep()`, `handle_glob()`
3. register in `src/tool/mod.rs`: add to `tool_definitions()` and `handle_tool_call()` dispatch
4. tests for both tools

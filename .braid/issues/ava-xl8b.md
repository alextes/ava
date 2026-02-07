---
schema_version: 9
id: ava-xl8b
title: design filesystem tool
priority: P2
status: done
type: design
deps: []
tags:
- tool
owner: null
created_at: 2026-02-07T19:12:35.363289Z
completed_at: 2026-02-07T19:13:01.871327Z
---

research and design for filesystem tool. see ava-uyvi for full writeup.

## decision: anthropic built-in + custom tools for other providers

use anthropic's built-in `text_editor_20250728` (schema-less, baked into model weights) when provider is anthropic. for other providers (e.g. OpenAI), define custom tool definitions with explicit JSON schemas.

## architecture

shared filesystem module with provider-agnostic operations:

```
anthropic API → "str_replace_based_edit_tool" ─┐
                                                ├→ fs::read_file(path, range)
openai API  → "read_file"  ────────────────────┘   fs::write_file(path, content)
                                                    fs::str_replace(path, old, new)
                                                    fs::insert(path, line, text)
                                                    fs::list_dir(path)
```

1. **shared fs module** — provider-agnostic file ops, path validation, safety checks
2. **provider-specific tool registration** — anthropic uses built-in schema-less tool, others use custom JSON schemas
3. **provider-specific input parsing** — match on tool name, translate to common fs ops

## design decisions

- **exact string replacement** for edits (industry consensus: claude code, openhands, aider)
- **content-based matching** not line numbers (line numbers break as files change)
- **actionable error feedback** on edit failures (critical for model self-correction)
- **approval required** for write/edit/create ops, auto-approve reads
- **path sandboxing** — validate against allowed directories, block path traversal
- **start anthropic-only** — add custom tools for OpenAI when ava-95z9 lands

## research sources

based on research into claude code, codex, openhands, aider, and cursor.
---
schema_version: 9
id: ava-yifv
title: add web_fetch tool (jina reader)
priority: P2
status: done
deps: []
tags:
- tool
- search
owner: null
created_at: 2026-02-07T17:59:21.730272Z
started_at: 2026-02-07T19:12:25.078567Z
completed_at: 2026-02-07T19:15:53.358985Z
---

add a web_fetch tool that fetches a URL and returns its content as clean text.

## tool interface

name: `web_fetch`
input:
- `url` (string, required) — URL to fetch
- `max_chars` (integer, optional, default 4000) — max output length

output: extracted text content from the page, truncated to max_chars.

if fetch fails: `failed to fetch URL: <error>`
if URL is invalid: `invalid URL: <url>`

## implementation

use jina reader API for zero-config HTML-to-text extraction:
- `GET https://r.jina.ai/<url>` — returns clean markdown/text
- no API key needed for basic usage
- set `Accept: text/plain` header for plain text output
- set a reasonable user-agent
- timeout: 30s
- truncate output to max_chars

fallback if jina is down: direct HTTP GET + basic HTML tag stripping (stretch goal, not required for MVP).

## considerations

- no approval required (read-only)
- could add URL validation (reject file://, localhost, internal IPs) as a safety measure
- jina handles PDFs, dynamic JS-rendered pages, etc. — a lot of value for free

## files

- `src/tool/mod.rs` — tool definition + handler + jina reader call
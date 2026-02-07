---
schema_version: 9
id: ava-tpsn
title: add web_search tool (brave search API)
priority: P2
status: done
deps: []
tags:
- tool
- search
owner: null
created_at: 2026-02-07T17:59:13.114354Z
started_at: 2026-02-07T18:51:34.565658Z
completed_at: 2026-02-07T18:53:48.700021Z
---

add a web_search tool that queries the brave search API.

## tool interface

name: `web_search`
input:
- `query` (string, required) — search query
- `max_results` (integer, optional, default 5, max 20)

output: formatted text results returned as tool result:
```
1. Title of Result
   https://example.com/page
   snippet text from the page...

2. Another Result
   https://example.com/other
   more snippet text...
```

if no results: `no results found for: <query>`
if API key missing: `web search unavailable: BRAVE_SEARCH_API_KEY not set`

## implementation

- brave search web search API: `GET https://api.search.brave.com/res/v1/web/search`
- auth: `X-Subscription-Token: <key>` header
- env var: `BRAVE_SEARCH_API_KEY`
- parse response JSON: `web.results[]` array with `title`, `url`, `description` fields
- truncate total output to 4000 chars (telegram compat)
- no approval required (read-only, auto-approve)
- add tool definition to `tool_definitions()` in `src/tool/mod.rs`
- add handler in `handle_tool_call()`
- gracefully handle missing API key (tool still registered, returns helpful error when invoked)

## files

- `src/tool/mod.rs` — tool definition + handler + brave API call
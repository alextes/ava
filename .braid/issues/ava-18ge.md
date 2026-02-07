---
schema_version: 9
id: ava-18ge
title: design web search and fetch tools
priority: P2
status: done
type: design
deps: []
tags:
- tool
- search
owner: null
created_at: 2026-02-07T17:53:03.616416Z
completed_at: 2026-02-07T18:06:11.415547Z
---

ava needs web search so the agent can look things up. there are two fundamentally different approaches and they're not mutually exclusive.

## approach 1: provider-specific server-side tools

anthropic offers a built-in `web_search_20250305` server-side tool type. it's not a regular tool — it's a special type in the API request that anthropic handles server-side using brave search.

### how it works

```json
{
  "tools": [{
    "type": "web_search_20250305",
    "name": "web_search",
    "max_uses": 5,
    "allowed_domains": ["docs.rs", "crates.io"],
    "blocked_domains": ["pinterest.com"]
  }]
}
```

claude decides when to search. response includes `server_tool_use` and `web_search_tool_result` content blocks with encrypted content + citations. the tool result gets passed back automatically in multi-turn — no client-side handling needed.

### implications for ava

- requires changes to the provider trait — tool definitions currently go through `tool_definitions()` which returns our custom `ToolDefinition` structs. server-side tools have a different shape (`type` field instead of `input_schema`)
- only works with anthropic. if we add openai provider (ava-95z9), their equivalent would be different
- $10 per 1,000 searches + token costs for the search results
- simplest integration — anthropic handles search, ranking, content extraction
- citations come back structured with `cited_text`, `url`, `title`

### what the provider trait needs

the current `Provider::complete()` takes `messages` and `system_prompt`. tool definitions are fetched inside `AnthropicProvider::complete()` via `tool::tool_definitions()`. server-side tools would need to be passed alongside regular tools but with a different structure. options:

1. **provider controls its own server-side tools** — anthropic provider adds `web_search` to the tools array internally, no trait change needed. downside: no way for the agent to enable/disable it per request.
2. **trait gains a `server_tools()` method** — providers declare what server-side tools they support. agent can toggle them.
3. **unified tool registry** — tool definitions become an enum of `ClientTool` vs `ServerTool`. providers map them to their API format.

## approach 2: provider-independent search tool

a regular tool (like exec) that the agent invokes, which calls a third-party search API. works with any provider.

### API options researched

| service | free tier | paid rate | notes |
|---------|-----------|-----------|-------|
| brave search | 2,000/mo | $5/1k | independent index, used by anthropic internally |
| serper | 2,500/mo | $1/1k | google results, very cheap at scale |
| tavily | 1,000/mo | $8/1k | purpose-built for AI agents, pre-filtered results |
| exa | ~2,000 (credit) | $5/1k | semantic/neural search, built in rust |
| jina reader | unlimited basic | token-based | content extraction, not search |

**recommendation: brave search** — generous free tier, independent index (no google dependency), the same engine anthropic uses for their server-side tool, simple REST API, privacy-focused.

### tool interface

```json
{
  "name": "web_search",
  "input": {
    "query": "string (required)",
    "max_results": "integer (optional, default 5)"
  }
}
```

returns formatted results:
```
1. Title of Result
   https://example.com/page
   snippet text from the page...

2. Another Result
   https://example.com/other
   more snippet text...
```

### companion: web_fetch tool

search finds URLs, but the agent often needs to read the actual page content. a `web_fetch` tool would:
- fetch a URL via HTTP GET
- convert HTML to plain text or markdown (could use jina reader API for free, or do it locally)
- truncate to 4000 chars (telegram compat)
- this is how claude code does it — WebSearch finds, WebFetch reads

## prior art

### openclaw
uses brave search API through its skill system. API key in config, direct REST calls. provider-independent.

### aidaemon
referenced in ava's memory design. focused on memory/facts, not search.

### goose (block)
uses MCP servers for extensibility. no built-in search — connects to tavily/brave via MCP.

### claude code
hybrid approach: server-side `WebSearch` (anthropic-only, brave-powered) + client-side `WebFetch` (fetches URLs locally, converts HTML to markdown, uses secondary LLM call to summarize). webfetch has 15min cache and 100KB limit.

### cline
web search locked to cline's own backend provider. not available with other providers.

### aider
no search — only URL scraping via playwright/httpx.

## architecture recommendation

**do both, start with provider-independent.**

1. **provider-independent search tool** (brave search API) — works with any provider, agent invokes it like exec. add a `BRAVE_SEARCH_API_KEY` env var. this is the MVP.

2. **provider-specific server-side tools** (later) — when we want the tighter integration and citations. requires provider trait changes to support the `web_search_20250305` tool type. this could be a follow-up once the openai provider (ava-95z9) is added and we need to think about provider-specific capabilities more broadly.

3. **web_fetch companion** — fetch and extract content from a specific URL. could use jina reader (`https://r.jina.ai/{url}`) for zero-config content extraction, or do it locally with a simple HTML-to-text conversion.

## decision

**go with provider-independent brave search API only.** no provider-specific server-side tools for now — we can always add them later if brave proves insufficient. keep it simple.

- `web_search` tool: brave search API, `BRAVE_SEARCH_API_KEY` env var, auto-approve (read-only, non-destructive)
- `web_fetch` tool: fetch URL content, jina reader for zero-config extraction
- no caching for MVP
- provider-specific tools deferred until there's a clear need

implementation issues: ava-tpsn (web_search), ava-yifv (web_fetch)
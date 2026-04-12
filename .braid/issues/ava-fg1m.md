---
schema_version: 9
id: ava-fg1m
title: integrate MCP tools into ava tool system
priority: P2
status: done
deps:
- ava-nfcu
- ava-i3o5
tags:
- extensibility
- mcp
owner: null
created_at: 2026-03-15T22:20:53.709754Z
started_at: 2026-03-16T09:02:46.035714Z
completed_at: 2026-03-16T09:10:17.051195Z
acceptance:
- MCP tools appear in tool_definitions() alongside built-in tools
- MCP tool calls are routed to the correct server
- MCP tool results are returned to the LLM as normal tool results
- built-in tool names cannot be shadowed by MCP tools
---

wire MCP-discovered tools into ava's existing tool definitions and dispatch.

## scope
- on startup, after MCP servers are initialized, call tools/list on each server
- convert MCP tool schemas to ava's ToolDefinition::Custom variant
- namespace tools as `mcp__<servername>__<toolname>` to avoid collisions with built-in tools
- extend handle_tool_call() to route MCP tool calls to the appropriate server's tools/call
- convert MCP tool results back to ava's MessageContent format

## non-goals (for now)
- MCP resources and prompts (tools only for v1)
- dynamic tool refresh (restart server to pick up new tools)
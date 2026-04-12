---
schema_version: 9
id: ava-nfcu
title: add MCP client transport (JSON-RPC 2.0 over stdio)
priority: P2
status: done
deps: []
tags:
- extensibility
- mcp
owner: null
created_at: 2026-03-15T22:19:13.401574Z
started_at: 2026-03-15T22:25:03.089002Z
completed_at: 2026-03-15T22:43:01.159686Z
acceptance:
- can spawn an MCP server subprocess and complete the initialize handshake
- can call tools/list and get back tool definitions
- can call tools/call and get back tool results
- errors from the server are mapped to ava error types
---

implement the core MCP client that communicates with MCP servers over stdin/stdout using JSON-RPC 2.0.

## scope
- new `src/mcp/` module
- JSON-RPC 2.0 message framing (request, response, notification)
- `initialize` handshake with capability negotiation
- `tools/list` to discover available tools
- `tools/call` to invoke a tool and return the result
- async transport using tokio (spawn child process, read/write stdin/stdout)
- proper error mapping to ava's error types

## references
- MCP spec: https://modelcontextprotocol.io/
- JSON-RPC 2.0: https://www.jsonrpc.org/specification
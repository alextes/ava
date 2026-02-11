---
schema_version: 9
id: ava-3ryb
title: design MCP integration
priority: P2
status: open
type: design
deps: []
tags:
- extensibility
owner: null
created_at: 2026-02-04T21:37:17.07543Z
started_at: 2026-02-10T14:14:35.551198Z
---

extend ava with Model Context Protocol servers for custom tools.

## aidaemon approach
- spawns MCP servers as subprocesses
- JSON-RPC 2.0 over stdin/stdout
- discovers tools via tools/list call
- wraps MCP tools as native ava tools
- error resilience: failed servers don't break others

## questions to consider
- which MCP servers are most valuable? (filesystem, sqlite, github)
- server lifecycle management (restart on crash?)
- tool namespace conflicts with built-in tools?
- authentication/secrets for MCP servers?
- should we support MCP resources and prompts too?

## references
- MCP spec: https://modelcontextprotocol.io/
- existing servers: https://github.com/modelcontextprotocol/servers

## output
- server configuration format
- tool discovery and wrapping
- error handling strategy
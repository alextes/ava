---
schema_version: 9
id: ava-i3o5
title: add MCP server config and lifecycle management
priority: P2
status: done
deps:
- ava-nfcu
tags:
- extensibility
- mcp
owner: null
created_at: 2026-03-15T22:19:34.316518Z
started_at: 2026-03-15T22:45:09.185037Z
completed_at: 2026-03-15T23:23:21.985016Z
acceptance:
- MCP servers are declared in a config file
- servers are spawned as subprocesses on startup
- crashed servers are restarted with backoff
- servers are shut down gracefully on ava exit
---

add configuration and process lifecycle management for MCP servers.

## scope
- config file format for declaring MCP servers (e.g. `~/.ava/mcp.toml` or section in a broader config)
  - server name, command, args, env vars
  - enable/disable flag
- spawn MCP server subprocesses on startup
- restart crashed servers with exponential backoff
- graceful shutdown on ava exit
- pass environment variables to server subprocesses (for auth/secrets)

## example config
```toml
[[mcp_servers]]
name = "github"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "..." }

[[mcp_servers]]
name = "sqlite"
command = "uvx"
args = ["mcp-server-sqlite", "--db-path", "./data.db"]
```
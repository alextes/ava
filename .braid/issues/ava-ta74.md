---
schema_version: 9
id: ava-ta74
title: research exec tool prior art across agent harnesses
priority: P2
status: open
type: design
deps: []
tags:
- tool
owner: null
created_at: 2026-02-08T19:51:29.948941Z
---

research how other agent harnesses handle exec/shell tools. compare approaches across:

- **claude code** — shell execution, approval model, sandboxing
- **openclaw** — tool/skill system
- **goose** (block) — MCP-based extensibility
- **cline** — terminal integration
- **aider** — shell command handling
- **openhands** (formerly opendevin) — sandbox and tool execution
- **codex** (openai) — shell execution model
- **cursor** — terminal and tool integration

focus areas:
- what commands/actions do they allow?
- how do they handle approval / sandboxing / safety?
- do they use a single exec tool or separate tools (file ops, shell, etc.)?
- timeout and resource limits
- output handling (truncation, streaming, etc.)

output: summary of approaches and recommendations for ava's exec tool improvements.
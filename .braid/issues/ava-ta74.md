---
schema_version: 9
id: ava-ta74
title: research exec tool prior art across agent harnesses
priority: P2
status: doing
type: design
deps: []
tags:
- tool
owner: agent-two
created_at: 2026-02-08T19:51:29.948941Z
started_at: 2026-02-08T19:54:03.885692Z
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

### specific design questions

1. **dangerous command blocking** — ava currently has a basic blocklist (`rm -rf /`, `mkfs`, fork bombs). should we expand it? what do other harnesses block? is a blocklist the right approach or is approval-based gating better? research what patterns others use (blocklist, allowlist, sandbox, approval tiers).

2. **working directory argument** — should the exec tool accept a `cwd` parameter to run commands in a specific directory? how do other harnesses handle this? (e.g. does claude code pass cwd, does it cd internally, does it maintain a stateful shell session with persistent cwd?)

output: summary of approaches and recommendations for ava's exec tool improvements.
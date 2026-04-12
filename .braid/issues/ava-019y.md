---
schema_version: 9
id: ava-019y
title: sealed execution with secret injection and output scrubbing
priority: P2
status: done
deps:
- ava-6q57
- ava-21is
tags:
- security
- secret
- tool
owner: null
created_at: 2026-03-19T08:42:28.577574Z
started_at: 2026-03-20T09:26:22.30628Z
completed_at: 2026-03-20T09:28:10.833116Z
---

add a sealed execution mode where the harness injects secrets as env vars and scrubs their values from command output before returning results to the agent.

## flow

1. skill activation resolves secret sources (vault:// reads file from ~/.ava/vault/)
2. harness requests approval via telegram for each secret
3. on approval, secrets are set as env vars on the Command
4. command executes normally
5. before returning output to agent, harness replaces any occurrence of secret values with [REDACTED]
6. secret values are never stored in conversation history or logs

## implementation

- add a sealed_exec function in src/tool/exec.rs (or a new module)
- accepts HashMap<String, String> of injected env vars
- after execution, scan stdout/stderr for injected values and replace
- the agent sees the command output but never the raw secrets
- env vars only exist for the lifetime of the command process

## edge cases

- multi-line secrets (e.g. PEM keys) — scrub each line independently
- secrets that appear in structured output (JSON, etc.) — simple string replacement handles this
- commands that fail — still scrub output before returning error
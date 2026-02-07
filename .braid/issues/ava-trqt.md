---
schema_version: 9
id: ava-trqt
title: design exec tool approval rules
priority: P2
status: done
type: design
deps:
- ava-obhq
tags:
- tool
- security
owner: null
created_at: 2026-02-07T11:50:55.819693Z
completed_at: 2026-02-07T18:03:16.545316Z
---

design a tool approval rules system so users can pre-approve exec tool invocations matching specific patterns, removing the need for per-invocation confirmation.

## key requirements

### command matching

- match the executable (the first token — the program being invoked)
- support glob-like pattern matching on arguments (e.g. "this flag can have any value", "this positional arg must be exactly X")
- support matching environment variables set at invocation time

### argument-level granularity

- each argument position can have its own matching rule: exact value, glob/wildcard, or unrestricted
- if a rule matches, the invocation is auto-approved; otherwise it requires manual approval
- rules should be strict by default — only explicitly allowed patterns pass

### pipe and chain awareness

- commands joined by `|`, `&&`, `||`, or `;` must each be evaluated independently
- a matching prefix does not auto-approve subsequent commands in the pipeline
- every command in a compound expression must individually match an approval rule for the full expression to be auto-approved

## prior art: claude code's `allowedTools`

claude code uses a pattern like `Bash(command prefix:*)` where `*` is a trailing wildcard. examples:

```
"Bash(git add:*)"           — any git add invocation
"Bash(cargo test:*)"        — any cargo test invocation
"Bash(kubectl get:*)"       — kubectl get with any args
"Bash(kubectl --context=* get:*)" — context flag with any value, then get subcommand
"Bash(KUBECONFIG=* kubectl get:*)" — env var prefix with any value
```

this format is a good starting point. observations:

- simple and readable — `command:*` covers most cases
- supports env var prefixes (`ENVVAR=* cmd`)
- supports flag wildcards in specific positions (`--flag=*`)
- limitation: only trailing wildcards — can't express "second arg must be X but third can be anything"
- limitation: no explicit pipe/chain handling — unclear if `git add . && git commit` matches `Bash(git add:*)`

our design should aim for similar simplicity for the common case while supporting more granular per-argument rules and explicit pipe/chain decomposition when needed.

## output

architecture doc covering rule format, matching semantics, pipe/chain handling, and how rules are stored and evaluated at invocation time.
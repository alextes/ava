---
schema_version: 9
id: ava-f0rq
title: design secondary audit agent for command safety classification
priority: P2
status: open
type: design
deps: []
tags:
- approval
- security
- design
owner: null
created_at: 2026-03-11T11:10:38.710953Z
---

research and design a secondary AI agent that audits commands before execution, providing a safety classification to complement pattern matching.

## motivation

pattern matching is inherently limited against bash's complexity. command substitution, backgrounding, newlines, variable expansion — there are many ways to hide malicious payloads inside pattern-matched commands. rather than trying to make pattern matching perfect (impossible with bash), add a second layer: an AI auditor.

## research questions

1. **how does claude code handle this?** claude code flags command substitution (`$()`, backticks) as requiring special approval with a heads-up. this is a simple heuristic that catches a large class of attacks. we should implement similar heuristics as a baseline.

2. **secondary audit agent design**: a cheaper/faster model (e.g. haiku) that receives the raw command and classifies risk:
   - `safe` — clearly benign (ls, cat, grep on local files)
   - `low` — standard dev commands (cargo build, git operations)
   - `medium` — unknown/unusual but not obviously harmful
   - `high` — suspicious patterns (network access, file deletion, encoded payloads)
   - `critical` — obviously harmful (rm -rf, credential exfil, reverse shells)

3. **hijack resistance**: the auditor sees only the raw command string, not the conversation context. this makes it hard to prompt-inject through the command itself (though `echo "ignore previous instructions..."` style attacks inside command substitution are a concern). the auditor's system prompt should be hardened and minimal.

4. **auto-approval tiers**: over time, `safe`/`low` classifications from the auditor could feed into auto-approval decisions, reducing prompt fatigue without relying solely on pattern matching.

5. **cost/latency tradeoff**: auditing every command adds latency and API cost. could cache classifications for identical commands, or only audit commands that pass pattern matching but contain suspicious characters (`$`, backtick, `&`, etc.).

## prior art to investigate
- claude code's command approval UX and heuristics
- other AI coding assistants' sandboxing approaches
- static analysis approaches to shell command safety

## immediate low-hanging fruit (not blocked on this design)
- add `&` as a split delimiter in `split_subcommands`
- flag/reject commands containing `$(`, backticks as requiring explicit approval (like claude code does)
- add newline as a command separator

## output
- design document with recommended approach
- implementation issues if we decide to proceed
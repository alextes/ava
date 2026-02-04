---
schema_version: 9
id: ava-n5p7
title: design skills system
priority: P2
status: open
type: design
deps: []
tags:
- core
owner: null
created_at: 2026-02-04T21:37:09.324262Z
---

markdown files that inject context-specific instructions based on triggers.

## aidaemon approach
- markdown files with YAML frontmatter in skills/ directory
- frontmatter: name, description, triggers (comma-separated keywords)
- two-stage activation: pattern match then LLM confirmation
- activated skills inject full body into system prompt
- fail-open: if LLM confirmation fails, activate anyway

## questions to consider
- is LLM confirmation worth the latency/cost?
- could use simpler regex or exact match triggers?
- skill composition - can skills reference other skills?
- skill parameters - can triggers pass data to skill body?
- user-defined vs built-in skills?
- skill priority/ordering when multiple match?

## output
- file format spec
- activation logic
- prompt injection strategy
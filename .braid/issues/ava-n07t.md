---
schema_version: 9
id: ava-n07t
title: research prompt injection detection in web_fetch responses
priority: P2
status: open
type: design
deps: []
tags:
- security
- web
owner: null
created_at: 2026-03-12T13:34:53.014593Z
---

research and design a detection layer that scans web_fetch (and potentially web_search) responses for prompt injection attempts before they reach the agent loop.

## motivation

web content is a hostile surface for agents. malicious pages can embed instructions designed to hijack tool-calling agents (e.g. 'ignore previous instructions', fake tool results, JSON-like structures that look like system messages). this is an active threat in the wild.

## research questions

- what are the known prompt injection patterns used against web agents?
- where is the right place to scan — in web_fetch itself, or a middleware layer before tool results are returned to the agent?
- how do we balance detection with false positive rate? (legitimate pages may contain AI-related content)
- should flagged content be: blocked entirely, sanitised, or passed through with a warning injected?
- are there existing libraries or heuristics worth building on?

## candidate signals to scan for

- 'ignore previous instructions' and variants
- 'you are now', 'your new instructions are' and similar persona hijack phrases
- tool-call-like JSON structures embedded in page content
- system prompt boundary markers (e.g. </s>, <|im_end|>, [INST] etc.)
- unusually structured content that looks like it was designed for an LLM

## output

recommendation on detection approach, where to integrate it, and what action to take on detection
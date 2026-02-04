---
schema_version: 9
id: ava-1kcc
title: design cost tracking
priority: P3
status: open
type: design
deps: []
tags:
- observability
owner: null
created_at: 2026-02-04T21:37:20.117461Z
---

track token usage and optionally enforce budgets.

## aidaemon approach
- token usage stats across sessions and models
- optional daily token budgets
- cost calculation based on model pricing

## questions to consider
- storage schema for usage data
- granularity: per-session, per-day, per-model?
- budget enforcement: hard stop vs warning?
- cost estimation before expensive operations?
- reporting/visualization?

## output
- tracking schema
- budget enforcement strategy
- reporting interface
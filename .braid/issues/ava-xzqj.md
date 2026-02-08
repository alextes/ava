---
schema_version: 9
id: ava-xzqj
title: add tests for approval flow
priority: P2
status: done
deps: []
tags:
- test
- approval
owner: null
created_at: 2026-02-08T18:43:22.699764Z
completed_at: 2026-02-08T18:47:56.507926Z
---

the approval system in src/approver.rs (231 lines) has zero tests. the agent's handle_tool_call_with_approval() approval routing is also untested.

## what to test

### approver.rs
- CliApprover always returns AutoApproved (currently tested in tool/mod.rs, could move here)
- AnyApprover dispatch to Cli variant
- pattern generation from commands (generate_pattern)

### agent approval routing (handle_tool_call_with_approval)
- tool requiring approval + AllowOnce → executes
- tool requiring approval + Deny → returns "command denied by user"
- tool requiring approval + AllowAlways → saves rule + executes
- tool not requiring approval → executes without asking

## approach

create a TestApprover that returns configurable decisions. test the agent's routing logic with it.

## files

- `src/approver.rs` — add `#[cfg(test)]` module
- `src/agent/mod.rs` — add approval routing tests
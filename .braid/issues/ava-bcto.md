---
schema_version: 9
id: ava-bcto
title: design self-service approval rule management tool
priority: P2
status: done
type: design
deps: []
tags:
- tool
- approval
owner: null
created_at: 2026-02-08T20:19:04.354398Z
started_at: 2026-02-08T20:19:06.564497Z
completed_at: 2026-02-08T20:24:53.967276Z
---

add an `action=add` to the existing `manage_rules` tool so the agent can propose new approval rules. the human sees the exact proposed pattern and approves or rejects it before it's saved.

## what exists

- `manage_rules` tool with `action=list` and `action=delete` (ava-g3m4, done)
- `requires_approval()` currently only triggers for `exec` tool
- `TelegramApprover` checks stored rules and prompts with approve/deny/allow-always buttons
- `Database::save_approval_rule(pattern)` for persisting rules
- `Database::find_matching_rule(command)` for checking rules (ava-gmi8, done)

## design: add `action=add` to `manage_rules`

### user-facing flow

1. user says "you keep asking about cargo commands, just approve those"
2. agent calls `manage_rules` with `{"action": "add", "pattern": "cargo *"}`
3. tool call requires approval — telegram shows: `proposed rule: cargo *` with approve/deny buttons
4. if approved, pattern is saved via `db.save_approval_rule(pattern)`
5. future `cargo test`, `cargo build`, etc. auto-approve silently

### implementation

**extend `requires_approval()`**: return true for `manage_rules` when `action=add`. this means the existing approval flow handles everything — no new UI needed.

```rust
pub fn requires_approval(tool_call: &ToolCall) -> bool {
    match tool_call.name.as_str() {
        EXEC_TOOL_NAME => true,
        MANAGE_RULES_TOOL_NAME => {
            // adding rules requires approval; listing/deleting don't
            tool_call.input.get("action")
                .and_then(|v| v.as_str())
                .is_some_and(|a| a == "add")
        }
        _ => false,
    }
}
```

**extend `manage_rules` handler**: add an `"add"` match arm that takes a `pattern` field and calls `db.save_approval_rule(&pattern)`.

**customize the approval prompt for rule proposals**: in `TelegramApprover::request_approval()`, detect `manage_rules` add calls and show `proposed rule: <pattern>` instead of `command: <command>`. no "allow always" button for rule additions (that would be meta-circular).

**extend the tool definition**: add `"add"` to the action enum and a `pattern` field.

### what about the approval UX?

the existing approval flow works perfectly here:
- **CLI** (`CliApprover`): auto-approves, no prompt (fine for local dev)
- **telegram** (`TelegramApprover`): shows inline keyboard with approve/deny — human sees exactly what pattern will be saved

no "allow always" button on rule-add proposals — you don't want to auto-approve future rule additions.

### edge cases

- duplicate pattern: `save_approval_rule` already handles this (ignores duplicates)
- empty/blank pattern: reject with error message
- overly broad pattern like `*`: technically valid but dangerous — let the human decide via approval

## files to change

| file | change |
|------|--------|
| `src/tool/mod.rs` | add `"add"` arm to `manage_rules` handler, extend definition, update `requires_approval()` |
| `src/approver.rs` | customize prompt text for `manage_rules` add calls |

## output

one implementation issue covering the `action=add` extension.
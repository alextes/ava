---
schema_version: 9
id: ava-zy2c
title: add time-limited approval rules (allow for 15 minutes)
priority: P2
status: done
deps: []
tags:
- approval
- security
owner: null
created_at: 2026-03-24T11:39:47.11296Z
started_at: 2026-03-24T11:47:17.178756Z
completed_at: 2026-03-24T11:50:50.434Z
---

add a time-limited "allow for 15 minutes" option alongside the existing permanent "always allow" in approval prompts.

### why

the current "always" rules are permanent — great for safe commands like `cargo *`, but too broad for task-specific permissions. when working on an external codebase, you want to batch-approve reads/writes for that session without permanently granting access. a 15-minute window matches the typical focus block for a task.

### design

**database: add `expires_at` to `approval_rules`**

```sql
ALTER TABLE approval_rules ADD COLUMN expires_at TEXT;
```

- null = permanent (existing behavior, no change)
- non-null = ISO 8601 timestamp after which the rule is ignored

**rule checking: filter and purge expired rules**

- `list_approval_rules()` runs `DELETE FROM approval_rules WHERE expires_at IS NOT NULL AND expires_at < datetime('now')` before the SELECT — expired rules are purged every time rules are checked
- `check_command_coverage()`, `find_matching_edit_rule()`, `find_matching_read_rule()` — all go through `list_approval_rules()`, so expiry and cleanup work everywhere automatically
- no separate cleanup job needed — dead rules never accumulate past one approval cycle

**telegram UX: add "15 min" buttons**

current button layout:
```
[ approve ] [ deny ]
[ always: cargo test * ] [ always: cargo * ]
```

new layout:
```
[ approve ] [ deny ]
[ 15 min: cargo test * ] [ 15 min: cargo * ]
[ always: cargo test * ] [ always: cargo * ]
```

- "15 min" buttons use the same pattern, but `save_approval_rule` receives an `expires_at` parameter
- callback data: `exec:{nonce}:timed:{idx}` (new action alongside `always`)

**approval decision flow:**

- `AllowAlways { pattern }` → permanent (existing)
- `AllowTimed { pattern, duration_secs }` → time-limited (new variant on `ApprovalDecision`)
- agent loop saves the rule with `expires_at = now + duration`

### implementation

1. **migration v10**: `ALTER TABLE approval_rules ADD COLUMN expires_at TEXT`
2. **`db/rules.rs`**: update `save_approval_rule()` to accept optional `expires_at`, filter expired in `list_approval_rules()`
3. **`tool/mod.rs`**: add `AllowTimed` variant to `ApprovalDecision`
4. **`approver.rs`**: add "15 min" button row, handle `timed` callback action, pass expiry to `save_approval_rule()`
5. **`agent/mod.rs`**: handle `AllowTimed` in the approval result processing (same as `AllowAlways` but with expiry)

### test plan

- rule with `expires_at` in the future → matches
- rule with `expires_at` in the past → does not match
- `save_approval_rule` with expiry stores the timestamp
- expired rules are cleaned up on list
- `AllowTimed` decision saves rule with correct expiry

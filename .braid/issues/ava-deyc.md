---
schema_version: 9
id: ava-deyc
title: add /rules command for listing and deleting approval rules
priority: P2
status: done
deps:
- ava-obhq
tags:
- tool
- approval
owner: null
created_at: 2026-02-07T19:08:15.904949Z
completed_at: 2026-02-20T16:12:30.855738Z
---

there's no user-facing way to view or manage saved approval rules. add a \`/rules\` command to the telegram bot.

## what to do

**\`/rules\`** (no args) — list all approval rules:
\`\`\`
approval rules:
1. cargo *
2. ls *
3. git push *
\`\`\`
if no rules: \`no approval rules saved.\`

**\`/rules delete <n>\`** — delete a rule by its displayed number:
\`\`\`
deleted rule: cargo *
\`\`\`

use existing \`db.list_approval_rules()\` and \`db.delete_approval_rule(id)\`.

## notes

- the numbers shown to the user should be 1-indexed for UX, mapped back to the actual DB IDs internally
- handle edge cases: invalid number, out of range, non-numeric input

## files

- \`src/main.rs\` or wherever telegram commands are dispatched — add /rules handling
- uses existing DB methods, no schema changes needed
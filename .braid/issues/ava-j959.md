---
schema_version: 9
id: ava-j959
title: handle approval timeout gracefully instead of propagating error
priority: P1
status: done
deps: []
tags:
- core
- agent
owner: null
created_at: 2026-02-10T08:08:44.71399Z
completed_at: 2026-02-10T08:09:21.948788Z
---

when telegram approval times out after 5 minutes, Error::ApprovalTimeout propagates via ? in handle_tool_call_with_approval, crashing process(). this leaves orphaned tool_use blocks in the DB without tool_result, corrupting future conversation turns (provider rejects malformed history). fix: catch ApprovalTimeout and return a tool_result telling the model the approval timed out, same pattern as Deny.
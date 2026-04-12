---
schema_version: 9
id: ava-ojva
title: add gmail tool with read auto-approve and send approval
priority: P2
status: open
deps: []
tags:
- tool
- gmail
owner: null
created_at: 2026-03-13T16:02:50.17181Z
---

add gmail as a native tool the agent can use. reference implementation in ~/operations/.claude/skills/gmail/ (GmailClient with OAuth via op run + 1Password).

read-only operations (inbox, search, read, download attachments) should be auto-approved but logged. sending and replying are high-impact — they must go through the same telegram approval flow as bash commands (the agent cannot bypass this).

scope:
- gmail_read tool: inbox, search, read message, download attachments — no approval needed, just log
- gmail_send tool: send + reply — requires approval via telegram (show recipient, subject, body preview)
- credentials: reuse the op run + 1Password pattern from the reference implementation
- two accounts: personal (alex.tesfamichael@gmail.com) and ultrasound (alex@ultrasound.money)

open question: the reference implementation uses `op run` to inject credentials from 1Password at runtime. this works fine for read operations, but for send approval via telegram callbacks the agent process needs credentials already available in-memory — it can't shell out to `op run` mid-callback. need to decide: (a) load credentials into memory on startup via `op run`, (b) store tokens differently, or (c) rethink how the send approval + execution flow works.
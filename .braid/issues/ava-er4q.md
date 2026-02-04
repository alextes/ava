---
schema_version: 9
id: ava-er4q
title: design session architecture
priority: P2
status: open
type: design
deps:
- ava-1q20
tags:
- session
owner: null
created_at: 2026-02-01T21:30:18.670091Z
---

ava needs a session mechanism that:

- persists across model switches (claude -> gpt -> claude)
- can continue API provider sessions using their session IDs when available
- can initialize new API sessions with context pulled from storage to continue the abstract conversation
- works across channels (telegram, cli message subcommand, future channels)

ava is intended as a single continuous entity — an identity template/mold someone can instantiate by cloning and giving a new identity.

output: architecture doc covering session storage, context management, and cross-provider continuity.
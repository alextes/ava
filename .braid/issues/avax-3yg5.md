---
schema_version: 9
id: avax-3yg5
title: show saved pattern in approval confirmation message
priority: P2
status: open
deps:
- ava-obhq
tags:
- tool
- approval
owner: null
created_at: 2026-02-07T19:08:04.204156Z
---

after pressing "allow always", the telegram message is edited to \`-> approved (always)\` with no mention of what pattern was saved. users have no idea what they just approved.

## what to do

change the confirmation message in \`TelegramApprover::handle_callback()\` to include the pattern:
- current: \`-> approved (always)\`
- new: \`-> approved (always for: cargo *)\`

the pattern needs to be available at callback time. currently, \`handle_callback\` receives the action string \`allow_always\` and the pattern is generated later on the approver side. two options:

1. encode the pattern in the callback data: \`exec:{nonce}:allow_always:{pattern}\` — then \`handle_callback\` can extract and display it. but this means generating the pattern before the callback (at keyboard-build time), which is cleaner anyway.
2. pass the pattern back through the oneshot channel and have the caller edit the message — messier.

option 1 is preferred and aligns with the phase 2 design where the callback data carries the pattern directly.

## files

- \`src/approver.rs\` — generate pattern at keyboard-build time, encode in callback data, display in confirmation
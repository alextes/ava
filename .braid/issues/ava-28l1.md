---
schema_version: 9
id: ava-28l1
title: design telegram-to-vault secret provisioning (agent-blind)
priority: P2
status: skip
type: design
deps:
- ava-o6p7
tags:
- security
- secret
- telegram
owner: null
created_at: 2026-03-20T09:24:19.601081Z
completed_at: 2026-03-27T11:14:49.371093Z
---

when the agent needs a secret placed in the vault, the user currently has to SSH into the host and write the file manually. this is friction that discourages proper secret management.

## idea

a special approval-like flow where the agent asks the user for a secret, the user provides it via telegram, but the value is intercepted by the harness and written directly to `~/.ava/vault/<name>` — never passing through the agent's conversation context.

## research questions

- can the harness intercept a telegram message before it reaches the agent queue? the telegram_producer currently forwards all text to the agent loop.
- how to signal that "the next message is a secret, not a normal message"? options: a callback button that opens an input prompt, a special `/secret <name>` command, or a two-step flow where the agent requests and the harness prompts.
- should the secret value be deleted from telegram chat history after capture? telegram bot API supports deleteMessage.
- how to handle the case where the user accidentally sends the secret as a normal message (it would end up in conversation history).
- could telegram's "self-destructing messages" or "secret chat" features help here?

## constraints

- the agent must never see the raw secret value
- the secret must not be stored in conversation history or logs
- the secret should be written to `~/.ava/vault/<name>` and confirmed to the agent as "secret <name> has been stored"
- the flow should work from the user's phone (no SSH needed)

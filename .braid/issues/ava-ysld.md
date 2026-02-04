---
schema_version: 9
id: ava-ysld
title: allow specifying provider in message command
priority: P2
status: open
deps:
- ava-95z9
owner: null
created_at: 2026-02-01T22:57:22.762642Z
---

add a --provider flag to the message command to select which provider to use.

```
ava message --provider openai "hello"
ava message --provider anthropic "hello"
```

in the future this would override the session-bound provider. for now, just use it to select the provider for the single message.

depends on having multiple providers available.
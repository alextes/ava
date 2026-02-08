---
schema_version: 9
id: ava-95z9
title: add openai provider
priority: P2
status: done
deps: []
owner: null
created_at: 2026-02-01T22:46:32.550273Z
completed_at: 2026-02-08T14:09:02.967392Z
---

implement an OpenAI provider following the same pattern as AnthropicProvider.

- default model: gpt-5.2
- use the OpenAI chat completions API
- read api key from OPENAI_API_KEY env var
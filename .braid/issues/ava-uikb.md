---
schema_version: 9
id: ava-uikb
title: add OpenRouter provider (chat completions API)
priority: P1
status: done
deps: []
tags:
- provider
owner: null
created_at: 2026-04-12T14:40:45.913579Z
started_at: 2026-04-12T14:40:50.159694Z
completed_at: 2026-04-12T14:44:26.901127Z
---

add a third provider variant for OpenRouter, using the OpenAI Chat Completions wire format. OpenRouter supports hundreds of models from different providers (Google, DeepSeek, Meta, Anthropic, OpenAI) through a unified API.

## key decisions (from discussion)

- provider name: "openrouter", models use OpenRouter's provider/model format (e.g. "google/gemini-2.5-flash")
- no model validation — accept any model string, let OpenRouter return errors for invalid ones
- always include cache_control breakpoints — providers that don't understand them ignore them
- env var: OPENROUTER_API_KEY
- default model: google/gemini-2.5-flash (cheap, capable, good default)
- chat completions wire format (POST /v1/chat/completions)

## caching behavior

- OpenAI/DeepSeek models: automatic server-side caching, no API changes needed
- Anthropic models: cache_control passthrough works via OpenRouter
- other models: no caching, but often cheap enough to not matter
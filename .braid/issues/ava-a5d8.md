---
schema_version: 9
id: ava-a5d8
title: add image content support to message and provider layer
priority: P2
status: open
deps: []
tags:
- tool
owner: null
created_at: 2026-02-08T19:41:54.559095Z
---

add an Image variant to MessageContent so tools can return images (e.g. browser screenshots).

### message layer (src/message.rs)

- add `Image { media_type: String, data: String }` variant to `MessageContent`
  - `data` is base64-encoded image bytes
  - `media_type` is e.g. `"image/png"`, `"image/jpeg"`
- add `MessageContent::image(media_type, data)` constructor
- update `ToolCallResult` to support returning image content (tool results need to carry a vec of content blocks, not a single string)

### anthropic provider (src/provider/anthropic.rs)

- update message serialization to emit anthropic image content blocks:
  `{ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "..." } }`
- tool results with images become multi-block content arrays

### openai provider (src/provider/openai.rs)

- update message serialization to emit openai image_url content:
  `{ "type": "image_url", "image_url": { "url": "data:image/png;base64,..." } }`
- tool results with images use content arrays

### dependencies

- add `base64` crate to Cargo.toml

### notes

- keep backward compatibility: existing text-only tools should work unchanged
- this is a prerequisite for the browser tool's screenshot action
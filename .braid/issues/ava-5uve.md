---
schema_version: 9
id: ava-5uve
title: add tests for anthropic provider response parsing
priority: P1
status: done
deps: []
tags:
- test
owner: null
created_at: 2026-02-01T22:57:49.39977Z
started_at: 2026-02-01T23:05:15.821892Z
completed_at: 2026-02-04T20:36:59.524863Z
---

the AnthropicProvider has no unit tests. this is the core API integration.

add tests for:
- parsing text content blocks
- parsing multiple text blocks (should join with newline)
- parsing tool_use content blocks
- handling API error responses
- request serialization

use serde_json to test parsing without hitting the network. example:

```rust
#[test]
fn test_parse_text_response() {
    let json = r#"{"content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn"}"#;
    let response: ApiResponse = serde_json::from_str(json).unwrap();
    // ...
}
```
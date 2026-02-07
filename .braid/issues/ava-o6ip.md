---
schema_version: 9
id: ava-o6ip
title: support built-in tool types in anthropic provider
priority: P2
status: open
deps: []
tags:
- tool
owner: null
created_at: 2026-02-07T19:12:47.296814Z
---

update the provider layer to support anthropic's schema-less built-in tools alongside regular custom tools.

currently ToolDefinition assumes all tools have a name, description, and input_schema. anthropic's built-in text_editor_20250728 is declared differently:

```json
{"type": "text_editor_20250728", "name": "str_replace_based_edit_tool"}
```

vs regular custom tools:

```json
{"name": "exec", "description": "...", "input_schema": {...}}
```

changes needed:

- extend ToolDefinition (or add a variant enum) to support both schema-less built-in tools and custom tools
- update AnthropicProvider API request serialization to emit the correct format for each tool type
- the tool_definitions() function needs to return both types
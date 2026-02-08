---
schema_version: 9
id: ava-by43
title: design character/persona customization
priority: P2
status: open
type: design
deps: []
tags:
- core
owner: null
created_at: 2026-02-08T16:23:41.313971Z
---

## goal

allow users to customize the bot's persona/character via a dedicated tool, separate from `remember_fact` (which stays for user info).

## proposed approach

### new `set_character` tool

add a `set_character` tool that takes a key and value, e.g.:
- key: "tone", value: "formal and precise"
- key: "name", value: "jarvis"
- key: "personality", value: "dry wit, concise"

this keeps character customization explicit and separate from user facts.

### storage

two options to evaluate:

1. **reuse facts table with a reserved category** (e.g. `"_character"`) — simple, no migration needed, but overloads the facts table semantics
2. **separate `character_traits` table** — cleaner separation, requires a migration, but makes the intent explicit

recommendation: option 1 (reserved category) for simplicity — the facts table already has category/key/value and upsert logic.

### system prompt integration

in `system_prompt()`, load character traits and render as a dedicated section between `DEFAULT_SYSTEM_PROMPT` and known facts:

```
<base system prompt>

## character
- tone: formal and precise
- personality: dry wit, concise

## known facts
...
```

character traits augment the base prompt, they don't replace it.

### `remember_fact` scope

update `remember_fact`'s tool description to clarify it's for user-related info only. add a note like: "use this to remember facts about the user. for bot persona/character settings, use set_character instead."

## open questions

- should there be a `list_character` / `get_character` tool, or is that overkill?
- should users be able to reset character traits (delete key)?
- max number of character traits to prevent prompt bloat?
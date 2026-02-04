---
schema_version: 9
id: ava-ad5o
title: design markdown to telegram HTML conversion
priority: P2
status: open
type: design
deps: []
tags:
- telegram
owner: null
created_at: 2026-02-04T20:46:30.29674Z
---

when we want richer formatting in telegram, we'll need to convert LLM markdown output to telegram HTML.

## context

telegram supports a limited HTML subset (see ava-tjj4 for full spec):
- `<b>`, `<i>`, `<u>`, `<s>`, `<code>`, `<pre>`, `<a>`, `<blockquote>`
- must escape `<`, `>`, `&`
- no `<br>` (use `\n`)
- malformed nesting = all formatting stripped

## approach options

### 1. regex-based conversion
simple find/replace for common patterns:
- `**text**` → `<b>text</b>`
- `*text*` → `<i>text</i>`
- `` `code` `` → `<code>code</code>`
- ```lang\ncode``` → `<pre language="lang">code</pre>`

pros: simple, no deps
cons: fragile with edge cases, nested formatting tricky

### 2. pulldown-cmark + custom renderer
parse markdown properly, emit telegram HTML:

```rust
use pulldown_cmark::{Parser, Event, Tag};

fn markdown_to_telegram_html(md: &str) -> String {
    let parser = Parser::new(md);
    let mut output = String::new();
    for event in parser {
        match event {
            Event::Start(Tag::Strong) => output.push_str("<b>"),
            Event::End(Tag::Strong) => output.push_str("</b>"),
            // ...
        }
    }
    output
}
```

pros: robust parsing, handles nesting
cons: adds dependency

### 3. ask model for telegram HTML directly
system prompt instructs model to output telegram-compatible HTML.

pros: no conversion needed
cons: models often get it wrong, hard to enforce

## recommendation

option 2 (pulldown-cmark) — robust and pulldown-cmark is a well-maintained, fast crate. the custom renderer is ~50 lines.

## output

- decision on approach
- implementation issue if approved
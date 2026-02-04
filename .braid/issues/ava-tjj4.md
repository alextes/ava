---
schema_version: 9
id: ava-tjj4
title: design telegram html formatting
priority: P2
status: done
type: design
deps: []
tags:
- telegram
owner: null
created_at: 2026-02-01T21:30:04.744663Z
started_at: 2026-02-01T23:07:04.496614Z
completed_at: 2026-02-04T20:46:34.134272Z
---

telegram's HTML parsing mode is quite involved. research and document:

- what tags are supported
- escaping requirements
- how to handle code blocks, links, formatting
- edge cases and gotchas

output: clear spec for implementing the formatter.

---

## design spec

### supported tags

| tag | purpose | notes |
|-----|---------|-------|
| `<b>`, `<strong>` | bold | |
| `<i>`, `<em>` | italic | |
| `<u>`, `<ins>` | underline | |
| `<s>`, `<strike>`, `<del>` | strikethrough | |
| `<code>` | inline code | |
| `<pre>` | code block | |
| `<pre language="rust">` | code block with syntax | language attr optional |
| `<a href="url">text</a>` | links | |
| `<a href="tg://user?id=123">` | user mention | |
| `<tg-spoiler>` | spoiler | also `<span class="tg-spoiler">` |
| `<tg-emoji emoji-id="123">` | custom emoji | |
| `<blockquote>` | block quote | |

**not supported:** `<br>` (use `\n`), custom fonts, colors, any other HTML tags.

### escaping requirements

**must escape these characters:**
- `<` → `&lt;`
- `>` → `&gt;`
- `&` → `&amp;`

only escape when not part of a tag or entity. numerical entities (`&#123;`) work. only named entities supported: `&lt;`, `&gt;`, `&amp;`, `&quot;`.

### code blocks

```html
<!-- inline -->
<code>let x = 1;</code>

<!-- block -->
<pre>
fn main() {
    println!("hello");
}
</pre>

<!-- block with language (syntax highlighting) -->
<pre language="rust">
fn main() {
    println!("hello");
}
</pre>
```

### links

```html
<a href="https://example.com">click here</a>
<a href="tg://user?id=123456789">@username</a>
```

### gotchas and edge cases

1. **tag nesting must be correct** — malformed nesting causes telegram to strip all formatting and show plain text
2. **UTF-16 length calculation** — entity offsets/lengths must be counted in UTF-16 code units, not bytes or codepoints
   - BMP chars (U+0000–U+FFFF): 1 unit
   - supplementary (emoji, etc): 2 units (surrogate pair)
3. **whitespace trimming** — entity length should exclude trailing whitespace, but offsets must include preceding whitespace
4. **no `<br>` tags** — use literal `\n` in the string
5. **nested entities supported** — can combine bold+italic, etc.

### implementation approach

create an `escape_html()` function:

```rust
pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
```

for LLM output, we may need to convert markdown to telegram HTML. options:
1. ask the model to output telegram HTML directly (fragile)
2. parse markdown and convert to telegram HTML (more robust)
3. use raw text without formatting (simplest, loses formatting)

**recommendation:** start with option 3 (plain text), add markdown→HTML conversion later if needed.

### sources

- https://core.telegram.org/bots/api
- https://core.telegram.org/api/entities
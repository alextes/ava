---
schema_version: 9
id: ava-i727
title: add browser tool with chromiumoxide
priority: P2
status: open
deps:
- ava-a5d8
tags:
- tool
owner: null
created_at: 2026-02-08T19:46:21.921075Z
---

add a `browser` tool using chromiumoxide for CDP-based browser automation.

### design decisions

- **single tool** with `action` parameter (not separate tools per action)
- **global static** browser instance via OnceLock/OnceCell — lazy-initialized, reused across calls
- **system chrome required** — look at known paths, error if not found (no auto-download)
- **headless by default** — works on servers/telegram. env var `AVA_BROWSER_VISIBLE=1` for debugging
- **text + image output** — text actions return strings, screenshot returns base64 image

### actions (core set)

- `navigate { url }` — go to URL, wait for page load, return page title
- `click { selector }` — click element by CSS selector, return confirmation
- `type { selector, text }` — type text into input element, return confirmation
- `screenshot` — capture full page screenshot, return as base64 image (requires image content support)
- `get_text { selector? }` — extract text content from page or specific element

### implementation

1. add `chromiumoxide` to Cargo.toml
2. add browser module (e.g. `src/tool/browser.rs`)
3. chrome binary detection: check known paths per platform
   - macOS: `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`
   - linux: `/usr/bin/google-chrome`, `/usr/bin/google-chrome-stable`
4. global browser instance: `static BROWSER: OnceLock<Browser>` or similar
   - lazy init on first browser tool call
   - configure headless, window size, etc.
5. tool definition with JSON schema for action enum + per-action params
6. register in `tool_definitions()` and dispatch in `handle_tool_call()`
7. URL validation (reuse existing `is_valid_url` pattern from web_fetch)

### resource limits

- 30s timeout per action
- single tab (close and reopen between navigations to prevent memory leaks)
- page text truncation (same 4000 char default as web_fetch)

### dependencies

- `chromiumoxide` crate (async, tokio, full CDP)
- image content support (ava-a5d8) for screenshot action
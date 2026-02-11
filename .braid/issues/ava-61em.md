---
schema_version: 9
id: ava-61em
title: add browser tool
priority: P2
status: open
type: meta
deps:
- ava-b13x
- ava-a5d8
- ava-i727
- ava-0p2x
- ava-n06u
tags:
- tool
owner: null
created_at: 2026-02-01T21:37:34.076221Z
started_at: 2026-02-08T19:21:31.106785Z
---

browser automation tool for the agent loop:

- control chrome via chrome devtools protocol (CDP)
- navigate, click, type, screenshot
- similar to claude code's browser tool

enables web interaction and research tasks.

## prior art research

### existing tool pattern in ava

tools follow a consistent pattern in `src/tool/mod.rs`:
- constant name + `ToolDefinition` with JSON schema
- input deserialization struct
- async implementation function
- registration in `tool_definitions()` vector
- dispatch in `handle_tool_call()` match

the `web_fetch` tool (jina reader) is the closest analog — HTTP fetch + content extraction. a browser tool handles JS-rendered pages and interactive automation.

### how others do it

**claude code — chrome + extension + CDP**

- chrome extension ("claude in chrome") installed from chrome web store — handles page interaction
- native messaging host binary bridges extension to CLI via stdio
- CDP used under the hood for navigate, click, screenshot, DOM, network
- controls the user's existing chrome session — preserves cookies, login state
- runs in a visible window (not headless) — user sees actions in real-time
- also supports playwright MCP as alternative — uses accessibility tree snapshots instead of pixel screenshots

**openclaw — browser relay + CDP**

- browser relay service with three-port architecture: gateway, control, relay
- three modes: extension relay (existing tabs), managed (isolated), remote CDP (cloud)
- typescript-based, not rust

**puppeteer / playwright (node.js)**

- puppeteer: google's CDP wrapper, chrome/chromium only
- playwright: microsoft's multi-browser tool, its own protocol layer over CDP
- both can use system chrome via executable path config

### rust CDP crates

**chromiumoxide** (recommended)
- ~984 stars, CDP, async/tokio, actively maintained (2025)
- auto-generates rust bindings from chromium's PDL (~60k lines)
- full CDP: navigate, click, type, screenshot, JS eval, network interception, PDF
- `BrowserFetcher` can auto-download chromium
- supports system chrome via `BrowserConfig::builder().chrome_executable("/path/to/chrome")`
- macOS: `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`
- fits naturally into ava's tokio runtime

**headless_chrome**
- ~1.6k stars, CDP, synchronous only (threads), actively maintained
- high-level puppeteer-style API, auto-downloads chromium
- **synchronous only — dealbreaker** for ava's async agent loop

**fantoccini**
- ~1.9k stars, WebDriver (not CDP), async/tokio, actively maintained
- requires external driver process (chromedriver/geckodriver)
- no CDP-specific features — less capable

**playwright-rs**
- ~500 stars, playwright JSON-RPC, async, moderate maintenance
- requires node.js playwright server — heavy dependency

### chrome vs chromium

most automation tools download chromium because it's open-source and freely redistributable.

practical differences:
- **DRM/widevine**: chrome has it, chromium doesn't
- **licensed codecs**: chrome includes H.264, AAC, MP3
- **auto-updates**: chrome auto-updates, chromium doesn't

for ava's use case (web research, form filling, scraping), either works fine. using system chrome is simpler — no download step, user already has it. chromiumoxide supports both via `chrome_executable()`.

### decision matrix

| criteria | chromiumoxide | headless_chrome | fantoccini | playwright-rs |
|----------|--------------|-----------------|------------|---------------|
| async/tokio | yes | **no** | yes | yes |
| system chrome | yes | no | n/a | n/a |
| setup complexity | medium | low | high | high (node.js) |
| full CDP features | yes | most | no | no |
| maintenance | active | active | active | moderate |
| external deps | chrome binary | auto-downloads | webdriver | node.js + playwright |

### recommendation

**chromiumoxide** is the best fit:
- async/tokio native — integrates with existing runtime
- full CDP access — navigate, click, type, screenshot, JS eval
- pure rust — no node.js dependency
- supports system chrome via `chrome_executable()` — prefer user's installed chrome, fall back to auto-download or error
- active maintenance

### design decisions (resolved)

- **tool shape**: single `browser` tool with `action` parameter
- **browser lifecycle**: global static instance (OnceLock), lazy-initialized on first call
- **chrome binary**: require system chrome at known paths, error if not found
- **output format**: text by default, screenshot action returns base64 image (requires image content support)
- **headless vs visible**: headless by default, `AVA_BROWSER_VISIBLE=1` env var for debugging
- **actions**: navigate, click, type, screenshot, get_text
- **resource limits**: 30s timeout per action, single tab, 4000 char text truncation

### implementation issues

- ava-a5d8: add image content support to message and provider layer
- ava-i727: add browser tool with chromiumoxide (depends on ava-a5d8)
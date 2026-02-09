---
schema_version: 9
id: ava-q2i6
title: share a single reqwest::Client across providers and web tools
priority: P2
status: done
deps: []
tags:
- core
- provider
owner: null
created_at: 2026-02-09T13:11:30.578537Z
started_at: 2026-02-09T13:14:26.972608Z
completed_at: 2026-02-09T13:20:16.870131Z
---

every inbound message triggers `provider_for_session()` which constructs a new provider, each calling `reqwest::Client::new()`. this creates a fresh connection pool and TLS session every time. the web tools (`web_search`, `web_fetch`) also create a new `reqwest::Client` on every invocation, even within a single tool loop.

`reqwest::Client` is designed to be reused — it's internally `Arc`-wrapped, cheap to clone, and holds a connection pool + TLS session cache. creating one at startup and sharing it avoids repeated TLS handshakes and enables HTTP/2 connection reuse.

## what to do

create a single `reqwest::Client` at startup and thread it through all paths that make HTTP requests.

### 1. provider constructors — `src/provider/anthropic.rs` + `src/provider/openai.rs`

- change `new(api_key)` → `new(client: Client, api_key: String)`
- change `from_env()` → `from_env(client: Client)`
- update test helpers that call `::new("test-key".into())`

### 2. `AnyProvider` — `src/provider/mod.rs`

- change `from_name(provider, model)` → `from_name(client: Client, provider, model)`
- change `default_from_env()` → `default_from_env(client: Client)`
- update tests

### 3. web tools — `src/tool/web.rs`

- change `web_search(query, max_results)` → `web_search(client: &Client, query, max_results)`
- change `web_fetch(url, max_chars)` → `web_fetch(client: &Client, url, max_chars)`
- change `handle_web_search(call)` → `handle_web_search(client: &Client, call)`
- change `handle_web_fetch(call)` → `handle_web_fetch(client: &Client, call)`
- remove the `reqwest::Client::new()` calls inside these functions
- update `test_web_search_missing_api_key` to pass a client

### 4. tool dispatch — `src/tool/mod.rs`

- change `handle_tool_call(db, call)` → `handle_tool_call(db, call, client: &Client)`
- pass `client` to `handle_web_search`, `handle_web_fetch`
- pass `client.clone()` to `AnyProvider::from_name()` in the `switch_model` branch

### 5. agent — `src/agent/mod.rs`

- add `client: reqwest::Client` field to `Agent` struct
- change `Agent::new(provider, approver, db)` → `Agent::new(provider, approver, db, client)`
- pass `&self.client` to `tool::handle_tool_call()` in `handle_tool_call_with_approval`

### 6. main — `src/main.rs`

- create `let http_client = reqwest::Client::new();` in `run_start()` and `run_message()`
- pass `http_client.clone()` through `agent_loop()`, `provider_for_session()`, `Agent::new()`

## files

- `src/provider/anthropic.rs` — constructor signature
- `src/provider/openai.rs` — constructor signature
- `src/provider/mod.rs` — `AnyProvider` methods + tests
- `src/tool/web.rs` — web functions + test
- `src/tool/mod.rs` — `handle_tool_call` signature + tests
- `src/agent/mod.rs` — `Agent` struct + tests
- `src/main.rs` — create client, thread through
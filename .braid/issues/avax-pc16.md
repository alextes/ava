---
schema_version: 9
id: avax-pc16
title: strip env var prefixes in pattern generation and matching
priority: P2
status: open
deps:
- ava-obhq
tags:
- tool
- approval
owner: null
created_at: 2026-02-07T19:07:54.294201Z
---

\`generate_pattern()\` and \`matches_single()\` break when commands have leading env var assignments like \`RUST_LOG=debug cargo test\`. the first token is \`RUST_LOG=debug\`, producing the useless pattern \`RUST_LOG=debug *\`.

## what to do

add a \`strip_env_prefix()\` function that removes leading \`KEY=VALUE\` tokens before pattern generation and matching. a token is an env var assignment if it matches \`^[A-Z_][A-Z0-9_]*=\`.

- use in \`generate_pattern()\`: strip before taking first/second tokens
- use in \`matches_single()\`: strip from both pattern and command before comparison
- add tests:
  - \`generate_pattern("RUST_LOG=debug cargo test")\` → \`cargo *\`
  - \`generate_pattern("A=1 B=2 ls -la")\` → \`ls *\`
  - \`matches_rule("cargo *", "RUST_LOG=debug cargo test")\` → true

## files

- \`src/db/mod.rs\` — add \`strip_env_prefix()\`, update \`generate_pattern()\` and \`matches_single()\`
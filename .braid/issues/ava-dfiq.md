---
schema_version: 9
id: ava-dfiq
title: offer narrow/broad pattern choices in approval keyboard
priority: P2
status: done
deps:
- ava-pc16
- ava-3yg5
- ava-obhq
tags:
- tool
- approval
owner: null
created_at: 2026-02-07T19:08:29.10242Z
completed_at: 2026-03-11T10:44:42.398934Z
---

replace the single "allow always" button with two scope options when a subcommand is detectable.

## what to do

after stripping env var prefixes (dep: ava-pc16), check if token[1] looks like a subcommand (alphabetic, doesn't start with \`-\`). if so, offer both narrow and broad patterns:

\`\`\`
command: cargo test -- --nocapture
[allow once] [deny]
[always: cargo test *] [always: cargo *]
\`\`\`

if token[1] is a flag or there's only one token, show just the broad pattern:
\`\`\`
command: ls -la /tmp
[allow once] [deny]
[always: ls *]
\`\`\`

**callback data format**: \`exec:{nonce}:always:{url_encoded_pattern}\`
- the pattern is determined at keyboard-build time and encoded in the callback data
- \`handle_callback\` decodes it and passes it through as \`AllowAlways { pattern }\`
- no more calling \`generate_pattern()\` on the approver side
- telegram limits callback_data to 64 bytes — patterns are well within this (longest realistic: ~40 bytes with prefix)

**examples after env stripping**:
- \`cargo test -- --nocapture\` → narrow: \`cargo test *\`, broad: \`cargo *\`
- \`git push origin main\` → narrow: \`git push *\`, broad: \`git *\`
- \`ls -la /tmp\` → broad only: \`ls *\`
- \`cargo --verbose test\` → broad only: \`cargo *\` (token[1] is a flag)
- \`RUST_LOG=debug cargo test\` → narrow: \`cargo test *\`, broad: \`cargo *\`

**keyboard layout**: use two rows — \`[allow once] [deny]\` on first row, pattern buttons on second row.

## files

- \`src/approver.rs\` — build keyboard with pattern options, handle new callback format
- depends on ava-pc16 (env stripping) and ava-3yg5 (pattern in callback data)
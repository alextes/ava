---
schema_version: 9
id: ava-zxfn
title: support self-upgrade for installed binaries (non-source)
priority: P2
status: open
deps: []
tags:
- daemon
- tool
owner: null
created_at: 2026-03-12T12:38:36.575923Z
---

the self_upgrade tool currently only works when running from a source checkout (uses CARGO_MANIFEST_DIR at compile time). for binaries installed via cargo-dist or other release methods, we need an alternative upgrade path.

options to investigate:
- download pre-built binary from github releases
- cargo install ava-agent --force
- self-replace the binary in-place

this is a separate concern from the source-based upgrade which is already working.
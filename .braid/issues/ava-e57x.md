---
schema_version: 9
id: ava-e57x
title: add binary self-update to ava upgrade
priority: P2
status: open
deps: []
tags:
- daemon
owner: null
created_at: 2026-03-09T13:55:08.098126Z
---

extend `ava upgrade` to support installed (non-source) binaries. when CARGO_MANIFEST_DIR doesn't exist (binary was installed via install.sh or cargo-dist), the command should:

1. detect current version via `env!("CARGO_PKG_VERSION")`
2. check github releases API for latest version
3. if newer version available, download the platform-appropriate binary (reuse logic from install.sh — detect arch, download tar.xz from releases)
4. replace the current binary (atomic rename)
5. signal the running process via SIGUSR1 if running

consider what cargo-dist already provides — the install.sh script already handles platform detection and binary download. the upgrade path could shell out to install.sh or reimplement the core logic in rust for better error handling.

also consider: version comparison, rollback on failure, and whether to prompt before upgrading.
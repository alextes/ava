---
schema_version: 9
id: ava-o6p7
title: secret management for skills
priority: P2
status: open
type: meta
deps:
- ava-6q57
- ava-21is
- ava-019y
- ava-h0s7
- ava-hsuq
tags:
- security
- skill
- secret
owner: null
created_at: 2026-03-19T08:41:59.455226Z
---

secret management for skills. two tiers based on sensitivity.

## what's done

- **medium sensitivity**: secrets stored in 1Password, resolved once at `ava start` via `op run` (one Touch ID prompt). injected as env vars on the daemon process. skills use them directly. output scrubbing catches accidental leaks.
- **skill secret declarations**: `secrets` field in SKILL.md frontmatter (name + source pairs). parsed but not yet wired into automatic injection — skills currently rely on env vars being present.
- **vault deny + sealed exec**: implemented as defense-in-depth. vault directory is hard-denied, sealed_exec scrubs secrets from output.

## what's remaining

- **high sensitivity (ava-hsuq)**: biometric approval for sensitive secrets via telegram mini app + BiometricManager API. design issue, not yet implemented.
---
schema_version: 9
id: ava-ijtt
title: add 1password CLI integration for high-sensitivity skill secrets
priority: P2
status: open
type: design
deps: []
tags:
- security
- secret
- skill
owner: null
created_at: 2026-03-20T12:17:09.684399Z
---

design how ava integrates with 1password CLI (`op`) for high-sensitivity secrets on macOS/linux systems where 1password is available.

## context

medium-sensitivity secrets live in `~/.ava/vault/` and are readable by skill scripts. high-sensitivity secrets should NOT be readable by any process the agent controls — they need biometric (touch ID) verification via 1password.

## key points

- `op read "op://vault/item/field"` prompts touch ID on macOS when 1password is locked
- the agent cannot modify 1password — it's an external app with its own security model
- this only works when the user is physically at the machine (not via phone/telegram)
- skill scripts could call `op read` directly, but the harness should still scrub the output
- the `op://` source type in skill frontmatter is already parsed but unused

## research questions

- should the harness call `op read` on behalf of the script, or let scripts call it directly?
- if scripts call it directly, the harness can't scrub values it doesn't know — should we read the same `op://` references to know what to scrub?
- how to handle the case where 1password is locked and the user is on their phone (not at the machine)? graceful error? queue for later?
- should there be an `ava vault import-from-op` command that copies 1p secrets to the local vault for when the user wants medium-sensitivity access without biometric?

## scope

- macOS and linux with 1password 8+ installed
- requires `op` CLI in PATH
- only works with local biometric (touch ID / fingerprint)
- does NOT work when user is on phone only

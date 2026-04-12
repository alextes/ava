---
schema_version: 9
id: ava-hsuq
title: design biometric approval for high-sensitivity secrets via mobile
priority: P2
status: open
type: design
deps: []
tags:
- security
- secret
- design
owner: null
created_at: 2026-03-19T08:42:57.447107Z
---

research and design a mechanism for high-sensitivity secret access that requires biometric authentication (touch ID / face ID), including when the user is on their phone.

## problem

medium-sensitivity secrets are gated by a telegram approval button — sufficient for secrets where the main concern is accidental or injected access. but for high-sensitivity secrets (production deploy keys, admin credentials), we want biometric proof that the human is present.

the current idea of triggering `op read` on the host works when the user is at their laptop (1Password prompts touch ID). but it fails when the user is on their phone — there's no way to trigger a host-side touch ID prompt remotely.

## research findings

### webauthn does NOT work in telegram mini apps

tested via web research (2026-03-19). WebAuthn / `navigator.credentials.get()` is non-functional in telegram's in-app webview on both platforms:

- **iOS**: telegram uses WKWebView. while iOS 16+ has partial WebAuthn support in WKWebView, the host app must explicitly integrate with Apple's ASAuthorization framework. telegram has not done this. calls fail silently or with `NotAllowedError`.
- **android**: telegram uses `android.webkit.WebView`, which google explicitly excludes from WebAuthn support. the `navigator.credentials` API is undefined or non-functional. WebAuthn only works in Chrome proper and Chrome Custom Tabs.

this rules out the WebAuthn approach entirely.

### telegram's native BiometricManager API — the real solution

telegram provides its own biometric API for mini apps, available since **Bot API 7.2 (march 2024)**:

- `window.Telegram.WebApp.BiometricManager`
- `BiometricManager.requestAccess()` — request permission to use biometrics
- `BiometricManager.authenticate({ reason: "..." })` — triggers face ID / touch ID / fingerprint natively
- works on both iOS and android, inside the mini app, no external browser needed
- does NOT produce a WebAuthn credential — it's a simple biometric presence check

docs: https://core.telegram.org/bots/webapps#biometricmanager

## recommended approach: telegram mini app + BiometricManager

### architecture

```
telegram chat
  └─ mini app (TWA, hosted on our domain or self-hosted)
       ├─ Telegram.WebApp.BiometricManager.authenticate()
       └─ TLS connection to ava host (websocket or HTTPS callback)
```

### flow

1. skill activation requests a high-sensitivity secret
2. ava host sends a telegram message with an inline button that opens the mini app
3. mini app displays: "ava wants DEPLOY_KEY for skill 'deploy-prod' — authenticate to approve"
4. mini app calls `BiometricManager.authenticate({ reason: "unlock DEPLOY_KEY" })`
5. iOS prompts face ID / android prompts fingerprint
6. on biometric success, mini app sends approval to ava host via TLS
7. host unlocks the secret, proceeds with sealed execution

### security considerations

- BiometricManager confirms biometric presence but does not produce a cryptographic assertion. the approval message from mini app to host needs its own authentication:
  - option A: telegram's `initData` HMAC validation (proves the request came from our mini app + this specific user). this is probably sufficient — an attacker would need to compromise both telegram's bot token AND the TLS channel.
  - option B: add a shared secret between mini app and host for signing approvals. belt-and-suspenders.
- the secret never leaves the host — only an "approved" signal travels over the wire
- the biometric check is per-activation, not persistent

### open questions

- **hosting**: the mini app needs HTTPS hosting. could be a static page on a small VPS, or cloudflare pages, or self-hosted on the ava host itself (if it has a public domain)
- **connectivity**: the mini app needs to reach the ava host. options:
  - ava host exposes a webhook endpoint (requires public IP or tunnel like cloudflare tunnel / ngrok)
  - relay server that both mini app and host connect to
  - ava host polls for approval responses from a shared store (simplest but adds latency)
- **BiometricManager.authenticate() behavior**: need to verify — does it work reliably on both platforms? what happens if the device has no biometric sensor? (likely falls back to device passcode)
- **setup UX**: user needs to configure the mini app as part of their telegram bot. how much of this can we automate?

## fallback approaches

### TOTP fallback

if biometric isn't available (no sensor, BiometricManager unsupported), fall back to requiring a TOTP code typed into the telegram chat. proves device possession, not biometric presence, but still better than a button tap.

### 1Password mobile — not viable

1Password Connect and service accounts don't support delegating unlock to the mobile app. the CLI requires local biometric. not viable for remote phone-based approval.

### push notification + iOS shortcut — fragile

an iOS shortcut triggered by push that prompts face ID and calls a webhook. technically possible but depends on iOS automation reliability. not robust enough for security-critical flows.

## constraints

- must work from both laptop and phone
- must not require the ava host to be physically accessible
- secret should never be transmitted over telegram (even encrypted)
- the secret should ideally stay in 1Password or the vault — the auth mechanism unlocks access, it doesn't transmit the secret itself

## output

- recommended approach with trade-offs
- implementation issues if we decide to proceed
---
schema_version: 9
id: ava-xpsb
title: explore phone-based biometric secret unlock via telegram mini app
priority: P3
status: open
type: design
deps: []
tags:
- security
- secret
- telegram
owner: null
created_at: 2026-03-20T12:18:19.620382Z
---

explore whether a telegram mini app on the user's phone could serve as a biometric unlock mechanism for high-sensitivity secrets, enabling remote secret access when the user isn't at the machine running the agent.

## the fundamental tension

the agent runs on machine A. the user is on their phone (device B). a high-sensitivity secret is needed. the secret must be decrypted, but:

- if the agent's harness can decrypt it, the agent could modify the harness to exfiltrate it
- if the phone decrypts it and sends it to the harness, the harness (controlled by the agent) receives the plaintext

## the decision we have to make

either:
1. **the agent CAN edit the harness** → passing secrets to the harness is unsafe, but this is how ava currently works (the agent has exec + file edit). in this model, phone-based biometric unlock is security theater — the agent could just patch the harness to log secrets.
2. **the agent CANNOT edit the harness** → the harness is a trusted boundary. passing secrets through it is fine. sealed execution + output scrubbing provides real security. phone biometric unlock becomes meaningful.

option 2 requires making the harness binary immutable (read-only, signed, or running as a different user). this is a significant architectural change.

## if we go with option 2 (trusted harness)

a telegram mini app could:
- hold a private key in the phone's secure enclave
- receive a "secret request" from the harness (via telegram bot API)
- prompt face ID / touch ID
- on success, decrypt the secret and send it to the harness over an encrypted channel
- the harness uses it for sealed execution, scrubs output, discards the secret

the secret is short-lived on the harness machine, and the harness is trusted not to exfiltrate.

## if we stay with option 1 (untrusted harness)

phone-based biometric only adds friction, not real security. the threat model becomes: "make prompt injection attacks harder by requiring biometric for each secret access, even though a sufficiently sophisticated attack could modify the harness." this is still useful as defense-in-depth — it raises the bar significantly.

## research questions

- can telegram mini apps access biometric APIs (face ID, fingerprint)?
- what's the encrypted channel between mini app and harness? (telegram bot API messages are not E2E encrypted)
- could webauthn/passkeys work instead of a custom mini app?
- what does "making the harness immutable" look like practically?
- is defense-in-depth (option 1 + biometric friction) good enough for most use cases?

## prior art

- 1password connect server (self-hosted, API-based secret access)
- duo mobile push authentication
- webauthn/FIDO2 for remote attestation

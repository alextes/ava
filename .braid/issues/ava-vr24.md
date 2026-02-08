---
schema_version: 9
id: ava-vr24
title: design speech-to-text for voice messages (parakeet v3)
priority: P2
status: open
type: design
deps: []
tags:
- voice
- tool
owner: null
created_at: 2026-02-08T19:20:59.650683Z
---

## goal

allow ava to decode voice messages using NVIDIA parakeet v3 (a local speech-to-text model), so that when a user sends a voice message via telegram, ava can transcribe and process it.

## key design questions

### self-bootstrapping setup

the interesting challenge: if parakeet v3 isn't installed on the system, ava should be able to figure out how to install it itself. this means:

- when a voice message arrives and STT isn't available, inject a system-level hint into the context explaining the situation and how to set it up
- ava can then use its exec tool to install dependencies (pip install nemo_toolkit, download model weights, etc.)
- this is essentially "system memory" — the agent knows it received a voice message but doesn't have the capability yet, and gets guidance on how to acquire it

### context injection approach

when a voice message arrives and STT is not set up:
1. detect that the message is a voice message (telegram provides audio file)
2. check if parakeet v3 / nemo toolkit is available on the system
3. if not available, inject into the system prompt or as a system message: "you received a voice message but speech-to-text is not configured. here's how to set it up on most systems: ..."
4. ava can then walk the user through setup or attempt it autonomously

### runtime flow (once set up)

1. telegram sends voice message → ava receives audio file
2. download/access the audio file
3. run parakeet v3 inference locally to get transcript
4. feed transcript as the user's message content
5. process normally

### open questions

- should the STT setup instructions live as a character/system memory, or be hardcoded as a fallback?
- how to detect if parakeet is installed? (try importing nemo, check for model files, etc.)
- should we support other STT backends as fallback? (whisper.cpp, etc.)
- audio format handling — telegram sends .ogg/.oga, parakeet expects wav/flac — need ffmpeg or similar
- should transcription happen in-process (python subprocess) or as a separate service?
- gpu vs cpu inference — parakeet v3 is fast on gpu but may need cpu fallback

## prior art

- telegram bot API provides `getFile` to download voice messages
- parakeet v3: `nvidia/parakeet-tdt-0.6b-v2` on huggingface, part of NVIDIA NeMo toolkit
- typical setup: `pip install nemo_toolkit[asr]`, then load model and transcribe
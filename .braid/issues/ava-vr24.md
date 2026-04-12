---
schema_version: 9
id: ava-vr24
title: design speech-to-text for voice messages (parakeet v3)
priority: P2
status: done
type: design
deps: []
tags:
- voice
- tool
owner: null
created_at: 2026-02-08T19:20:59.650683Z
started_at: 2026-03-17T20:39:30.042932Z
completed_at: 2026-03-18T21:28:11.8839Z
---

## goal

allow ava to decode voice messages using parakeet-mlx (local STT on Apple Silicon), so that when a user sends a voice message via telegram, ava can transcribe and process it.

## design

### decisions
- **runtime**: parakeet-mlx on Apple Silicon. other platforms unsupported for now.
- **self-bootstrap**: if parakeet-mlx CLI isn't found, the transcribe tool returns install instructions. the agent can use exec to install it (requires approval).
- **pipeline**: tool-based. voice messages are queued with a hint; agent uses `transcribe` tool to get text.
- **CLI interface**: shell out to `parakeet-mlx <path> --format txt`. start simple, timestamps (--format json) can be added later.
- **no approval needed**: transcribe is read-only. the bootstrap install goes through exec which already requires approval.

### flow
1. telegram voice message arrives → telegram_producer downloads audio to `~/.ava/voice/<file_id>.ogg`
2. queued as `[voice message received — audio saved to /path/to/file.ogg. use the transcribe tool to get the text.]`
3. agent calls `transcribe` tool with the file path
4. tool shells out to `parakeet-mlx`, returns transcript
5. agent processes the transcript as if the user typed it

### scope
- `src/telegram.rs` — Voice type, get_file/download_file methods
- `src/commands/start.rs` — handle voice messages in telegram_producer
- `src/tool/transcribe.rs` — new transcribe tool
- `src/config.rs` — voice_dir() helper

### implementation issues
1. add voice message types and file download to TelegramBot
2. handle voice messages in telegram_producer (download + queue with hint)
3. add transcribe tool (shells out to parakeet-mlx CLI)

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
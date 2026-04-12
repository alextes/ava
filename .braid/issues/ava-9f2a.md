---
schema_version: 9
id: ava-9f2a
title: add speak tool (piper TTS with telegram voice messages and local playback)
priority: P2
status: done
deps:
- ava-7zcb
tags:
- tool
- voice
owner: null
created_at: 2026-03-24T16:32:29.333086Z
started_at: 2026-03-25T12:54:15.92297Z
completed_at: 2026-03-27T10:26:03.797242Z
---

add a `speak` tool that converts text to audio via piper TTS. on telegram, sends a voice message. locally, plays through system audio.

### tool definition

```json
{
  "name": "speak",
  "input": { "text": "string (required) — text to speak" }
}
```

### pipeline

1. check piper binary in PATH — error with install instructions if missing
2. check model exists at `~/.ava/tts/en_US-lessac-medium.onnx` — error with download instructions if missing
3. pipe text through piper → WAV output
4. **telegram**: pipe WAV through ffmpeg → OGG Opus bytes → `send_voice()` (requires ffmpeg in PATH)
5. **local**: pipe WAV to `afplay` (macOS) or `aplay` (linux)

### long text handling

- if text > 1000 chars, truncate spoken text to 1000 chars
- return the full text as the tool result regardless

### implementation

- `src/tool/speak.rs` — new module
- tool definition + input struct + handler
- `detect_piper()` — check PATH, check model directory
- `synthesize()` — spawn piper, capture WAV bytes
- `to_ogg_opus()` — spawn ffmpeg, pipe WAV → OGG
- `play_local()` — platform detection, spawn afplay/aplay
- register in `tool_definitions()` and `handle_tool_call()`
- speak tool needs access to the telegram bot + chat_id to send voice messages — route through the channel layer or return audio bytes as an attachment

### test plan

- unit: tool definition schema is correct
- unit: text truncation for long input
- unit: piper not found returns helpful error
- unit: ffmpeg not found returns helpful error (telegram path only)
- integration: synthesize + play locally (requires piper installed)
- integration: synthesize + send voice message (requires piper + ffmpeg + telegram)

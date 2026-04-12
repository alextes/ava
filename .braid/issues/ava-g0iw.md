---
schema_version: 9
id: ava-g0iw
title: design text-to-speech audio output (piper TTS)
priority: P2
status: done
type: design
deps: []
tags:
- voice
- tool
owner: null
created_at: 2026-02-08T19:21:20.289644Z
started_at: 2026-03-24T15:44:22.530232Z
completed_at: 2026-03-24T16:32:34.823998Z
---

## goal

allow ava to send voice messages on telegram and optionally play audio locally using piper TTS. everything stays on-device.

## research findings

### piper TTS

- **CLI**: `echo "text" | piper --model en_US-lessac-medium.onnx --output_file out.wav`
- **raw output**: `--output-raw` writes 16-bit signed PCM, mono, 22050 Hz to stdout — enables piping
- **latency**: 50-200ms per sentence on modern CPUs. cold start ~200-500ms for model load
- **json input mode**: `--json-input` reads JSONL from stdin, keeps model loaded — ideal for a daemon
- **no OGG output** — piper only outputs WAV or raw PCM. need ffmpeg/opusenc for telegram's OGG Opus format

### voice models

- **recommended default**: `en_US-lessac-medium` — widely considered best quality-to-speed ratio
- **quality tiers**: low (~15-30 MB, robotic), medium (~60-80 MB, natural), high (~100-200 MB, slightly better than medium)
- **medium is the sweet spot** — high is marginal improvement for 2x size
- **source**: hugging face `rhasspy/piper-voices` or github releases

### telegram voice messages

- **endpoint**: `sendVoice` (POST, multipart upload)
- **required format**: OGG with Opus codec (`.ogg`) — displayed as inline voice message with waveform
- **max size**: 50 MB (a minute of Opus is ~60-120 KB, so no concern)
- **pipeline**: piper WAV → ffmpeg OGG Opus → sendVoice upload

### local playback

- **macOS**: `afplay output.wav` (built-in, simple)
- **linux**: `aplay output.wav` (ALSA) or raw pipe: `piper --output-raw | aplay -r 22050 -f S16_LE -t raw -c 1`
- raw piping on macOS needs `sox`/`play` which isn't built-in

### installation

- **simplest**: `pip install piper-tts` — works on macOS + linux, handles onnxruntime
- **standalone binary**: github releases, extract + run
- **no homebrew formula** in homebrew-core

## design decisions

### tool shape: `speak` tool

a `speak` tool that the agent can call explicitly, not automatic TTS on all responses. reasons:

- most responses don't need audio — text is fine for code, lists, structured output
- the agent knows when speaking makes sense (greeting, summary, answer to a spoken question)
- keeps the common path fast (no ffmpeg overhead on every response)
- simple to implement — same pattern as other tools

```json
{ "action": "speak", "text": "here's what I found..." }
```

### telegram: voice messages via sendVoice

when the speak tool is called in a telegram context, send the audio as a voice message instead of (or in addition to) the text response. the user sees a playable waveform inline.

pipeline (no temp files):
```bash
echo "<text>" | piper --model model.onnx --output_file /dev/stdout 2>/dev/null | ffmpeg -i pipe:0 -c:a libopus -b:a 48k -f ogg pipe:1
```
capture stdout bytes in rust, send via `sendVoice` multipart upload.

### local playback: platform-detect + shell out

when running locally (CLI mode), play via system audio:
- detect OS at runtime
- macOS: pipe WAV to `afplay`
- linux: pipe raw PCM to `aplay`

### model management

- default model: `en_US-lessac-medium`
- model directory: `~/.ava/tts/` — piper model files (`.onnx` + `.onnx.json`)
- if model not found, return a helpful error telling the user how to install: `pip install piper-tts` and download the model
- future: ava could self-bootstrap using exec tool

### dependencies

- **piper** binary must be in PATH (user installs via pip or standalone binary)
- **ffmpeg** must be in PATH (for WAV → OGG Opus conversion, telegram only)
- both are external — ava shells out, no rust bindings needed

### long responses

- if text > 1000 chars, truncate to first 1000 chars for the spoken version with a note
- the full text response still goes through as text
- keeps voice messages short and useful (under ~30 seconds of audio)

## implementation issues

1. **add `speak` tool** — tool definition, input parsing, dispatch
2. **piper integration** — spawn piper process, capture WAV/raw output
3. **ffmpeg OGG conversion** — pipe piper output through ffmpeg for telegram
4. **sendVoice in TelegramBot** — multipart file upload for OGG voice messages
5. **local playback** — platform detection, shell out to afplay/aplay
6. **model detection** — check `~/.ava/tts/` for model files, helpful error if missing

### what's NOT in scope

- automatic TTS on all responses (would need a separate config system)
- voice model download/management (just error + instructions for now)
- streaming playback (batch is fine — under 2s for typical responses)
- piper daemon mode / keeping model loaded (optimization for later)

## resolved questions

- **tool shape**: `speak` tool — confirmed. agent calls it explicitly, only speaks when asked.
- **default voice**: `en_US-lessac-medium` — pending listening test. sample: https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/samples/speaker_0.mp3
- **proactive speaking**: no. only speak when user asks ava to start speaking. on telegram, user says "start speaking" and ava uses the speak tool until told to stop. text is still stored in db normally.
- **model directory**: `~/.ava/tts/` — confirmed.
- **speaking mode on telegram**: user toggles speaking mode in conversation. messages in the db should have some indication they were delivered as voice rather than text.
- **local voice I/O channel** (mic + headphones as bidirectional channel): separate design issue ava-yafn.

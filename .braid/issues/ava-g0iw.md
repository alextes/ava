---
schema_version: 9
id: ava-g0iw
title: design text-to-speech audio output (piper TTS)
priority: P2
status: open
type: design
deps: []
tags:
- voice
- tool
owner: null
created_at: 2026-02-08T19:21:20.289644Z
---

## goal

allow ava to speak responses aloud using local text-to-speech, outputting to an audio device connected to the system. uses piper TTS (or similar local TTS engine) so everything stays on-device.

## concept

if ava is running on a system with an audio output device (speakers, headphones, bluetooth), it could speak its responses in addition to (or instead of) text. this would be useful for:

- hands-free interaction
- accessibility
- home assistant / kiosk setups
- IoT / embedded deployments

## key design questions

### TTS engine

piper TTS (https://github.com/rhasspy/piper) is a good candidate:
- fast local inference, runs on CPU
- many voice models available
- simple CLI: `echo "text" | piper --model en_US-lessac-medium --output_file out.wav`
- can also stream to stdout for direct playback

other options to evaluate: espeak-ng, coqui TTS, bark

### audio output

- detect available audio output devices on the system
- play generated audio via system audio (aplay, paplay, afplay depending on OS)
- or stream directly: `piper ... --output-raw | aplay -r 22050 -f S16_LE -t raw -`

### activation model

- always speak? only when asked? configurable per-channel?
- probably should be a character trait / setting: `audio_output: enabled`
- telegram responses would stay text-only; audio output makes sense for CLI or local deployments
- could also send voice messages back on telegram via sendVoice API

### self-bootstrapping (same pattern as STT)

- if TTS isn't installed and audio output is requested, inject setup guidance
- piper install: download binary + voice model, or `pip install piper-tts`
- ava could set this up itself using exec tool

### open questions

- which piper voice model(s) to default to?
- how to detect if an audio output device is available and working?
- latency — should we stream word-by-word or generate full response then play?
- should this be a tool (`speak`) or automatic behavior based on config?
- interaction with telegram — send voice messages back? or text-only for telegram?
- how to handle long responses — summarize before speaking?

## prior art

- piper TTS: fast, local, many voices, MIT licensed
- home assistant uses piper for local voice
- typical flow: text → piper → wav → play via system audio
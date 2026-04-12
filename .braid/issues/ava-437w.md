---
schema_version: 9
id: ava-437w
title: research higher-quality TTS voice models (alternatives to piper defaults)
priority: P2
status: open
type: design
deps: []
tags:
- voice
- design
owner: null
created_at: 2026-03-24T16:26:47.959783Z
---

## goal

find a higher-quality local TTS voice than the default piper voices. the rhasspy/piper voices sound too synthetic. we want something more natural while staying on-device.

## directions to explore

- **custom piper models** — piper supports ONNX models trained on any dataset. are there community-trained models with better voices? (e.g. trained on higher-quality datasets like VCTK, LJSpeech alternatives)
- **Coqui TTS / XTTS** — multi-speaker, voice cloning, higher quality than piper. heavier but still local. does it have a simple CLI?
- **Bark** (suno-ai) — very natural sounding, supports emotion/tone. heavy (GPU preferred). is there a CPU-viable version?
- **StyleTTS 2** — state-of-the-art open TTS. quality rivals commercial APIs. GPU-heavy?
- **MLX TTS models** — anything optimized for apple silicon via MLX? (similar to how parakeet-mlx handles STT)
- **Parler TTS** — describe the voice you want in text, model generates it. interesting approach.
- **Kokoro** — lightweight, high-quality, Apache licensed. worth evaluating.
- **commercial API fallback** — ElevenLabs, OpenAI TTS, etc. as optional non-local backend for when quality matters more than privacy

## constraints

- must run locally (on-device, no cloud required)
- CPU inference acceptable if latency < 2s for a sentence
- apple silicon (M-series) is the primary target
- simple CLI or python script interface (ava shells out)
- MIT/Apache/similar license preferred

## deliverable

recommendation with voice samples, latency benchmarks, and install instructions. update ava-g0iw with the chosen model.
